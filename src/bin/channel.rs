use netchat::{cdma, walsh};
use std::collections::{BTreeMap, HashMap};
use std::env;
#[cfg(windows)]
use std::ffi::c_void;
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone)]
struct Logger {
    file: Arc<Mutex<File>>,
    path: PathBuf,
}

impl Logger {
    fn new(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)?;
        Ok(Self {
            file: Arc::new(Mutex::new(file)),
            path,
        })
    }

    fn write(&self, message: &str) {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if let Ok(mut file) = self.file.lock() {
            let _ = writeln!(file, "[{}] {}", seconds, message);
            let _ = file.flush();
        }
    }
}

struct Group {
    owner: String,
    members: HashMap<String, usize>,
    order: usize,
    exchanging: Option<String>,
}

impl Group {
    fn code(&self, member: &str) -> Vec<i8> {
        walsh::codes(self.order).swap_remove(self.members[member])
    }
}

struct Bus {
    hosts: HashMap<String, TcpStream>,
    groups: BTreeMap<u32, Group>,
    joined: HashMap<String, u32>,
    next_group: u32,
    logger: Logger,
}

impl Bus {
    fn new(logger: Logger) -> Self {
        Self {
            hosts: HashMap::new(),
            groups: BTreeMap::new(),
            joined: HashMap::new(),
            next_group: 0,
            logger,
        }
    }

    fn send(&mut self, to: &str, message: &str) {
        if let Some(stream) = self.hosts.get_mut(to) {
            let _ = writeln!(stream, "{}", message);
        }
    }

    fn list_groups(&mut self, me: &str) {
        let list: Vec<String> = self
            .groups
            .iter()
            .map(|(id, group)| format!("{},{},{}", id, group.owner, group.members.len()))
            .collect();
        self.send(me, &format!("GROUPS {}", list.join(" ")));
    }

    fn create(&mut self, me: &str) {
        if self.joined.contains_key(me) {
            self.send(me, "ERR you are already in a group");
            return;
        }
        self.next_group += 1;
        let id = self.next_group;
        let group = Group {
            owner: me.to_string(),
            members: HashMap::from([(me.to_string(), 0)]),
            order: 1,
            exchanging: Some(me.to_string()),
        };
        let code = group.code(me);
        self.groups.insert(id, group);
        self.joined.insert(me.to_string(), id);
        self.send(me, &format!("JOINED {} {}", id, walsh::format(&code)));
        self.logger.write(&format!(
            "{} created group {} with Walsh code {}",
            me,
            id,
            walsh::display(&code)
        ));
    }

    fn join(&mut self, me: &str, target: &str) {
        if self.joined.contains_key(me) {
            self.send(me, "ERR you are already in a group");
            return;
        }
        let id = target
            .parse::<u32>()
            .ok()
            .filter(|id| self.groups.contains_key(id))
            .or_else(|| {
                self.groups
                    .iter()
                    .find(|(_, group)| group.owner == target)
                    .map(|(id, _)| *id)
            });
        let Some(id) = id else {
            self.send(me, "ERR group not found, use group number or owner ip:port");
            return;
        };
        let group = self.groups.get_mut(&id).unwrap();
        if group.exchanging.is_some() {
            self.send(
                me,
                &format!("ERR group {} is exchanging Walsh codes, try again", id),
            );
            return;
        }
        let slot = (0..)
            .find(|slot| !group.members.values().any(|used| used == slot))
            .unwrap();
        let mut others: Vec<String> = group.members.keys().cloned().collect();
        others.sort();
        group.members.insert(me.to_string(), slot);
        group.exchanging = Some(me.to_string());
        self.joined.insert(me.to_string(), id);
        self.reorder(id);
        let code = self.groups[&id].code(me);
        self.send(
            me,
            &format!(
                "JOINED {} {} {}",
                id,
                walsh::format(&code),
                others.join(" ")
            ),
        );
        self.logger.write(&format!(
            "{} joined group {} with Walsh code {}",
            me,
            id,
            walsh::display(&code)
        ));
    }

    fn reorder(&mut self, id: u32) {
        let Some(group) = self.groups.get_mut(&id) else {
            return;
        };
        let order = group
            .members
            .values()
            .max()
            .map_or(1, |slot| (slot + 1).next_power_of_two());
        if order == group.order {
            return;
        }
        group.order = order;
        let members: Vec<String> = group.members.keys().cloned().collect();
        for member in members {
            self.send(&member, &format!("ORDER {}", order));
        }
        self.logger.write(&format!(
            "group {} Walsh code length changed to {}",
            id, order
        ));
    }

    fn relay(&mut self, me: &str, command: &str, argument: &str) {
        let Some((to, code)) = argument.split_once(' ') else {
            return;
        };
        let Some(id) = self.joined.get(me).copied() else {
            return;
        };
        if self.joined.get(to) != Some(&id) {
            self.send(me, &format!("GONE {}", to));
            return;
        }
        self.send(to, &format!("{} {} {}", command, me, code));
        self.logger.write(&format!(
            "{} sent its Walsh code to {} in group {}",
            me, to, id
        ));
    }

    fn ready(&mut self, me: &str) {
        let Some(id) = self.joined.get(me).copied() else {
            return;
        };
        if let Some(group) = self.groups.get_mut(&id)
            && group.exchanging.as_deref() == Some(me)
        {
            group.exchanging = None;
            self.logger.write(&format!(
                "{} finished Walsh code exchange in group {}",
                me, id
            ));
        }
    }

    fn signal(&mut self, me: &str, argument: &str) {
        let Some(id) = self.joined.get(me).copied() else {
            self.send(me, "ERR you are not in a group");
            return;
        };
        let Some((order, chips)) = argument.split_once(' ') else {
            self.send(me, "ERR invalid CDMA signal");
            return;
        };
        let Some(signal) = cdma::parse_signal(chips) else {
            self.send(me, "ERR invalid CDMA signal");
            return;
        };
        let current = self.groups[&id].order;
        if order.parse::<usize>().ok() != Some(current) {
            self.send(
                me,
                &format!(
                    "ERR Walsh code length changed to {}, send the message again",
                    current
                ),
            );
            return;
        }
        let others: Vec<String> = self.groups[&id]
            .members
            .keys()
            .filter(|member| member.as_str() != me)
            .cloned()
            .collect();
        for other in others {
            self.send(&other, &format!("SIGNAL {}", chips));
        }
        self.logger.write(&format!(
            "{} sent {} CDMA chips to group {}",
            me,
            signal.len(),
            id
        ));
    }

    fn leave_group(&mut self, me: &str) -> Option<u32> {
        let id = self.joined.remove(me)?;
        let group = self.groups.get_mut(&id)?;
        group.members.remove(me);
        if group.exchanging.as_deref() == Some(me) {
            group.exchanging = None;
        }
        let owner = group.owner == me;
        let others: Vec<String> = group.members.keys().cloned().collect();
        if owner {
            self.groups.remove(&id);
            for other in others {
                self.joined.remove(&other);
                self.send(&other, &format!("CLOSED {}", id));
            }
            self.logger.write(&format!("{} closed group {}", me, id));
        } else {
            for other in others {
                self.send(&other, &format!("GONE {}", me));
            }
            self.logger.write(&format!("{} left group {}", me, id));
            self.reorder(id);
        }
        Some(id)
    }

    fn leave(&mut self, me: &str) {
        self.leave_group(me);
        self.hosts.remove(me);
        self.logger.write(&format!("{} left the channel", me));
    }

    fn close(&mut self) {
        for stream in self.hosts.values() {
            let _ = stream.shutdown(Shutdown::Both);
        }
        self.hosts.clear();
        self.groups.clear();
        self.joined.clear();
    }
}

struct Server {
    stop: Arc<AtomicBool>,
    bus: Arc<Mutex<Bus>>,
    thread: Option<JoinHandle<()>>,
    port: u16,
}

impl Server {
    fn show_groups(&self) {
        let bus = self.bus.lock().unwrap();
        if bus.groups.is_empty() {
            println!("no groups");
            return;
        }
        for (id, group) in &bus.groups {
            println!(
                "group {} owner {} code length {}",
                id, group.owner, group.order
            );
            let mut names: Vec<&String> = group.members.keys().collect();
            names.sort();
            for name in names {
                println!("  {} {}", name, walsh::display(&group.code(name)));
            }
        }
    }

    fn close(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Ok(mut bus) = self.bus.lock() {
            bus.logger.write("channel stopped");
            bus.close();
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn main() {
    let port = read_port();
    let logger = Logger::new("channel.log").unwrap_or_else(|error| {
        eprintln!("could not create channel.log: {}", error);
        process::exit(1);
    });
    let mut password = String::new();
    let mut server: Option<Server> = None;
    println!("CDMA channel shell, type help");

    loop {
        print!("channel> ");
        let _ = io::stdout().flush();
        let mut line = String::new();
        if io::stdin().read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        match line.trim() {
            "help" => show_help(),
            "groups" => {
                if let Some(active) = &server {
                    active.show_groups();
                } else {
                    println!("channel is not running");
                }
            }
            "password" => {
                if server.is_some() {
                    println!("stop the channel before changing the password");
                } else if let Some(value) = masked_password() {
                    if value.is_empty() {
                        println!("password cannot be empty");
                    } else {
                        password = value;
                        println!("password set");
                    }
                }
            }
            "start" => {
                if let Some(active) = &server {
                    println!("channel is already open on port {}", active.port);
                } else if password.is_empty() {
                    println!("set a password first");
                } else {
                    match start_server(port, password.clone(), logger.clone()) {
                        Ok(started) => {
                            println!("CDMA channel opened on port {}", started.port);
                            server = Some(started);
                        }
                        Err(error) => println!("{}", error),
                    }
                }
            }
            "stop" => {
                if let Some(mut active) = server.take() {
                    active.close();
                    println!("channel stopped");
                } else {
                    println!("channel is not running");
                }
            }
            "logs" => show_logs(&logger.path),
            "exit" => break,
            "" => {}
            command => println!("unknown command '{}', type help", command),
        }
    }

    if let Some(mut server) = server {
        server.close();
    }
    logger.write("channel closed");
    println!("channel closed");
}

fn read_port() -> u16 {
    let arguments: Vec<String> = env::args().collect();
    if arguments.len() == 1 {
        return 9000;
    }
    if arguments.len() == 3 && arguments[1] == "-p" {
        return arguments[2].parse().unwrap_or_else(|_| usage());
    }
    usage()
}

fn usage() -> ! {
    eprintln!("usage: channel [-p port]");
    process::exit(1);
}

fn show_help() {
    println!("help       show all commands");
    println!("password   set the channel password");
    println!("start      open the CDMA channel");
    println!("stop       stop the channel, keep the shell open");
    println!("groups     show groups and member Walsh codes");
    println!("logs       follow logs, Ctrl+C returns here");
    println!("exit       close the channel shell");
}

fn masked_password() -> Option<String> {
    netchat::password::read_masked("enter password: ")
}

fn show_logs(path: &Path) {
    println!("showing logs, press Ctrl+C to return");
    let Ok(mut file) = File::open(path) else {
        println!("no logs available");
        return;
    };
    let mut position = 0;
    follow_log(&mut file, &mut position);
    println!();
}

fn print_new_logs(file: &mut File, position: &mut u64) {
    let _ = file.seek(SeekFrom::Start(*position));
    let mut new_text = String::new();
    if file.read_to_string(&mut new_text).is_ok() && !new_text.is_empty() {
        print!("{}", new_text);
        let _ = io::stdout().flush();
        *position += new_text.len() as u64;
    }
}

#[cfg(windows)]
struct ConsoleInput {
    handle: *mut c_void,
    original_mode: u32,
}

#[cfg(windows)]
impl ConsoleInput {
    fn new() -> Option<Self> {
        let handle = unsafe { GetStdHandle((-10i32) as u32) };
        if handle.is_null() || handle as isize == -1 {
            return None;
        }
        let mut original_mode = 0;
        if unsafe { GetConsoleMode(handle, &mut original_mode) } == 0 {
            return None;
        }
        let raw_mode = original_mode & !0x0001 & !0x0002 & !0x0004;
        if unsafe { SetConsoleMode(handle, raw_mode) } == 0 {
            return None;
        }
        Some(Self {
            handle,
            original_mode,
        })
    }

    fn ready(&self, milliseconds: u32) -> bool {
        unsafe { WaitForSingleObject(self.handle, milliseconds) == 0 }
    }

    fn read(&self) -> Option<u16> {
        let mut value = 0u16;
        let mut read = 0u32;
        let ok = unsafe {
            ReadConsoleW(
                self.handle,
                &mut value,
                1,
                &mut read,
                std::ptr::null_mut(),
            )
        };
        (ok != 0 && read == 1).then_some(value)
    }
}

#[cfg(windows)]
impl Drop for ConsoleInput {
    fn drop(&mut self) {
        unsafe {
            SetConsoleMode(self.handle, self.original_mode);
        }
    }
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetStdHandle(number: u32) -> *mut c_void;
    fn GetConsoleMode(handle: *mut c_void, mode: *mut u32) -> i32;
    fn SetConsoleMode(handle: *mut c_void, mode: u32) -> i32;
    fn WaitForSingleObject(handle: *mut c_void, milliseconds: u32) -> u32;
    fn ReadConsoleW(
        handle: *mut c_void,
        buffer: *mut u16,
        length: u32,
        read: *mut u32,
        reserved: *mut c_void,
    ) -> i32;
}

#[cfg(windows)]
fn follow_log(file: &mut File, position: &mut u64) {
    let Some(console) = ConsoleInput::new() else {
        return;
    };
    loop {
        print_new_logs(file, position);
        if console.ready(100) && console.read() == Some(3) {
            break;
        }
    }
}

#[cfg(not(windows))]
fn follow_log(file: &mut File, position: &mut u64) {
    loop {
        print_new_logs(file, position);
        thread::sleep(Duration::from_millis(100));
    }
}

fn bind_near(port: u16) -> Result<(TcpListener, u16), String> {
    for candidate in nearby(port) {
        if let Ok(listener) = TcpListener::bind(("0.0.0.0", candidate)) {
            listener
                .set_nonblocking(true)
                .map_err(|error| error.to_string())?;
            return Ok((listener, candidate));
        }
    }
    Err(format!("no free port near {}", port))
}

fn nearby(port: u16) -> Vec<u16> {
    let mut ports = vec![port];
    for distance in 1..=100 {
        ports.extend(port.checked_add(distance));
        ports.extend(port.checked_sub(distance).filter(|value| *value > 0));
    }
    ports
}

fn start_server(
    requested_port: u16,
    password: String,
    logger: Logger,
) -> Result<Server, String> {
    let (listener, port) = bind_near(requested_port)?;
    let stop = Arc::new(AtomicBool::new(false));
    let bus = Arc::new(Mutex::new(Bus::new(logger.clone())));
    let thread_stop = stop.clone();
    let thread_bus = bus.clone();
    let thread = thread::spawn(move || {
        logger.write(&format!("CDMA channel opened on port {}", port));
        while !thread_stop.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((stream, _)) => {
                    if let Err(error) = stream.set_nonblocking(false) {
                        logger.write(&format!("connection error: {}", error));
                        continue;
                    }
                    let bus = thread_bus.clone();
                    let password = password.clone();
                    let logger = logger.clone();
                    thread::spawn(move || handle(stream, bus, password, logger));
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(50));
                }
                Err(error) => {
                    logger.write(&format!("listener error: {}", error));
                    thread::sleep(Duration::from_millis(50));
                }
            }
        }
    });
    Ok(Server {
        stop,
        bus,
        thread: Some(thread),
        port,
    })
}

fn handle(stream: TcpStream, bus: Arc<Mutex<Bus>>, password: String, logger: Logger) {
    let Ok(address) = stream.peer_addr() else {
        return;
    };
    let me = address.to_string();
    let Ok(mut output) = stream.try_clone() else {
        return;
    };
    let mut lines = BufReader::new(stream).lines();
    let given = lines.next().and_then(|line| line.ok()).unwrap_or_default();
    if given.trim() != password {
        let _ = writeln!(output, "WRONG");
        logger.write(&format!("{} entered wrong password, rejected", me));
        return;
    }
    let _ = writeln!(output, "OK {}", me);
    bus.lock().unwrap().hosts.insert(me.clone(), output);
    logger.write(&format!("{} joined the CDMA channel", me));

    for line in lines {
        let Ok(line) = line else {
            break;
        };
        let (command, argument) = line.split_once(' ').unwrap_or((&line, ""));
        let argument = argument.trim();
        let mut bus = bus.lock().unwrap();
        match command {
            "HOSTS" => {
                let mut list: Vec<String> = bus.hosts.keys().cloned().collect();
                list.sort();
                bus.send(&me, &format!("HOSTS {}", list.join(" ")));
            }
            "GROUPS" => bus.list_groups(&me),
            "CREATE" => bus.create(&me),
            "JOIN" => bus.join(&me, argument),
            "HELLO" | "REPLY" => bus.relay(&me, command, argument),
            "READY" => bus.ready(&me),
            "SIGNAL" => bus.signal(&me, argument),
            "LEAVE" => match bus.leave_group(&me) {
                Some(id) => bus.send(&me, &format!("LEFT {}", id)),
                None => bus.send(&me, "ERR you are not in a group"),
            },
            _ => {}
        }
    }
    bus.lock().unwrap().leave(&me);
}

use netchat::csma::{Algorithm, Medium};
use netchat::stats::analyze;
use std::collections::HashMap;
use std::env;
#[cfg(windows)]
use std::ffi::c_void;
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{self, Command};
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

struct Bus {
    hosts: HashMap<String, TcpStream>,
    peers: HashMap<String, String>,
    requests: HashMap<String, String>,
    logger: Logger,
}

impl Bus {
    fn new(logger: Logger) -> Self {
        Self {
            hosts: HashMap::new(),
            peers: HashMap::new(),
            requests: HashMap::new(),
            logger,
        }
    }

    fn send(&mut self, to: &str, msg: &str) {
        if let Some(stream) = self.hosts.get_mut(to) {
            let _ = writeln!(stream, "{}", msg);
        }
    }

    fn broadcast(&mut self, msg: &str) {
        for stream in self.hosts.values_mut() {
            let _ = writeln!(stream, "{}", msg);
        }
    }

    fn find(&self, name: &str) -> Option<String> {
        if self.hosts.contains_key(name) {
            return Some(name.to_string());
        }
        let found: Vec<&String> = self
            .hosts
            .keys()
            .filter(|host| host.rsplit_once(':').map(|(ip, _)| ip) == Some(name))
            .collect();
        if found.len() == 1 {
            Some(found[0].clone())
        } else {
            None
        }
    }

    fn busy(&self, host: &str) -> bool {
        self.peers.contains_key(host)
            || self.requests.contains_key(host)
            || self.requests.values().any(|value| value == host)
    }

    fn leave(&mut self, me: &str) {
        self.hosts.remove(me);
        let mut others = Vec::new();
        if let Some(peer) = self.peers.remove(me) {
            self.peers.remove(&peer);
            others.push(peer);
        }
        if let Some(peer) = self.requests.remove(me) {
            others.push(peer);
        }
        let target = self
            .requests
            .iter()
            .find(|(_, value)| value.as_str() == me)
            .map(|(key, _)| key.clone());
        if let Some(target) = target {
            self.requests.remove(&target);
            others.push(target);
        }
        for other in others {
            self.send(&other, &format!("LEFT {}", me));
        }
        self.logger.write(&format!("{} left the channel", me));
    }

    fn close(&mut self) {
        for stream in self.hosts.values() {
            let _ = stream.shutdown(Shutdown::Both);
        }
        self.hosts.clear();
    }
}

struct Server {
    stop: Arc<AtomicBool>,
    bus: Arc<Mutex<Bus>>,
    algorithm: Arc<Mutex<Algorithm>>,
    thread: Option<JoinHandle<()>>,
    port: u16,
}

impl Server {
    fn switch(&self, algorithm: Algorithm) {
        *self.algorithm.lock().unwrap() = algorithm;
        if let Ok(mut bus) = self.bus.lock() {
            bus.broadcast(&format!("ALGO {}", algorithm));
            bus.logger
                .write(&format!("channel switched to {}", algorithm));
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
    let mut algorithm = None;
    let mut password = String::new();
    let mut server: Option<Server> = None;
    println!("channel shell, type help");

    loop {
        print!("channel> ");
        let _ = io::stdout().flush();
        let mut line = String::new();
        if io::stdin().read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let mut parts = line.split_whitespace();
        match parts.next() {
            Some("help") => show_help(),
            Some("show") if parts.next() == Some("all") => show_algorithms(algorithm),
            Some("stats") => show_statistics(Algorithm::all()),
            Some("stat") => {
                let value = parts.collect::<Vec<_>>().join(" ");
                match Algorithm::parse(&value) {
                    Some(selected) => show_statistics([selected]),
                    None => println!("algorithm not found, type show all"),
                }
            }
            Some("test") => run_tests(),
            Some("switch") => {
                let value = parts.collect::<Vec<_>>().join(" ");
                match Algorithm::parse(&value) {
                    Some(selected) => {
                        algorithm = Some(selected);
                        if let Some(active) = &server {
                            active.switch(selected);
                            println!("channel switched to {}", selected);
                        } else {
                            println!("using {} for the next start", selected);
                        }
                    }
                    None => println!("algorithm not found, type show all"),
                }
            }
            Some("use") => {
                if server.is_some() {
                    println!("use switch <name or number> while the channel is running");
                } else {
                    let value = parts.collect::<Vec<_>>().join(" ");
                    match Algorithm::parse(&value) {
                        Some(selected) => {
                            algorithm = Some(selected);
                            println!("using {}", selected);
                        }
                        None => println!("algorithm not found, type show all"),
                    }
                }
            }
            Some("password") => {
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
            Some("start") => {
                if let Some(active) = &server {
                    println!("channel is already open on port {}", active.port);
                } else if password.is_empty() {
                    println!("set a password first");
                } else if let Some(selected) = algorithm {
                    match start_server(port, selected, password.clone(), logger.clone()) {
                        Ok(started) => {
                            println!(
                                "channel opened on port {} using {}",
                                started.port, selected
                            );
                            server = Some(started);
                        }
                        Err(error) => println!("{}", error),
                    }
                } else {
                    println!("choose an algorithm first, type show all");
                }
            }
            Some("stop") => {
                if let Some(mut active) = server.take() {
                    active.close();
                    println!("channel stopped");
                } else {
                    println!("channel is not running");
                }
            }
            Some("logs") => show_logs(&logger.path),
            Some("exit") => break,
            Some(command) => println!("unknown command '{}', type help", command),
            None => {}
        }
    }

    if let Some(mut server) = server {
        server.close();
    }
    logger.write("channel closed");
    println!("channel closed");
}

fn read_port() -> u16 {
    let args: Vec<String> = env::args().collect();
    if args.len() == 1 {
        return 9000;
    }
    if args.len() == 3 && args[1] == "-p" {
        return args[2].parse().unwrap_or_else(|_| usage());
    }
    usage()
}

fn usage() -> ! {
    eprintln!("usage: channel [-p port]");
    process::exit(1);
}

fn show_help() {
    println!("help                 show all commands");
    println!("show all             show available algorithms");
    println!("use <name or number> choose an algorithm");
    println!("switch <name or number> change the running algorithm");
    println!("stats                show statistics for all algorithms");
    println!("stat <name or number> show statistics for one algorithm");
    println!("test                 run all tests");
    println!("password             set the channel password");
    println!("start                open the channel");
    println!("stop                 stop the channel, keep the shell open");
    println!("logs                 follow logs, Ctrl+C returns here");
    println!("exit                 close the channel");
}

fn show_algorithms(selected: Option<Algorithm>) {
    for (index, algorithm) in Algorithm::all().into_iter().enumerate() {
        if selected == Some(algorithm) {
            println!("{}. {} (selected)", index + 1, algorithm);
        } else {
            println!("{}. {}", index + 1, algorithm);
        }
    }
}

fn show_statistics<const N: usize>(algorithms: [Algorithm; N]) {
    for algorithm in algorithms {
        println!();
        println!("{}", algorithm);
        println!(
            "{:<10} {:<12} {:<18} {:<12}",
            "stations", "collisions", "avg delay (ms)", "throughput (%)"
        );
        for result in analyze(algorithm) {
            println!(
                "{:<10} {:<12} {:<18.2} {:<12.2}",
                result.stations,
                result.collisions,
                result.average_delay_ms,
                result.throughput_percent
            );
        }
    }
}

fn run_tests() {
    println!("running tests...");
    match Command::new("cargo")
        .arg("test")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .status()
    {
        Ok(status) if status.success() => println!("all tests passed"),
        Ok(_) => println!("tests failed"),
        Err(error) => println!("could not run tests: {}", error),
    }
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
    algorithm: Algorithm,
    password: String,
    logger: Logger,
) -> Result<Server, String> {
    let (listener, port) = bind_near(requested_port)?;
    let stop = Arc::new(AtomicBool::new(false));
    let bus = Arc::new(Mutex::new(Bus::new(logger.clone())));
    let medium = Arc::new(Medium::new());
    let current_algorithm = Arc::new(Mutex::new(algorithm));
    let thread_stop = stop.clone();
    let thread_bus = bus.clone();
    let thread_algorithm = current_algorithm.clone();
    let thread = thread::spawn(move || {
        logger.write(&format!(
            "channel opened on port {} using {}",
            port, algorithm
        ));
        while !thread_stop.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((stream, _)) => {
                    if let Err(error) = stream.set_nonblocking(false) {
                        logger.write(&format!("connection error: {}", error));
                        continue;
                    }
                    let bus = thread_bus.clone();
                    let medium = medium.clone();
                    let password = password.clone();
                    let logger = logger.clone();
                    let algorithm = thread_algorithm.clone();
                    thread::spawn(move || {
                        handle(stream, bus, password, algorithm, medium, logger)
                    });
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
        algorithm: current_algorithm,
        thread: Some(thread),
        port,
    })
}

fn handle(
    stream: TcpStream,
    bus: Arc<Mutex<Bus>>,
    password: String,
    algorithm: Arc<Mutex<Algorithm>>,
    medium: Arc<Medium>,
    logger: Logger,
) {
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
    let selected = *algorithm.lock().unwrap();
    let _ = writeln!(output, "OK {}", me);
    let _ = writeln!(output, "ALGO {}", selected);
    bus.lock().unwrap().hosts.insert(me.clone(), output);
    logger.write(&format!("{} joined using {}", me, selected));

    for line in lines {
        let Ok(line) = line else {
            break;
        };
        let (command, argument) = line.split_once(' ').unwrap_or((&line, ""));
        if command == "MSG" {
            let selected = *algorithm.lock().unwrap();
            let (_access, report) = medium.acquire(selected);
            logger.write(&format!(
                "{} got channel with {} after {} attempt(s), {} collision(s), {} ms wait",
                me, selected, report.attempts, report.collisions, report.waited_ms
            ));
            thread::sleep(Duration::from_millis(30));
            let mut bus = bus.lock().unwrap();
            if let Some(peer) = bus.peers.get(&me).cloned() {
                bus.send(&peer, &format!("MSG {} {}", me, argument));
                bus.logger
                    .write(&format!("{} sent message to {}", me, peer));
            }
            continue;
        }

        let mut bus = bus.lock().unwrap();
        match command {
            "HOSTS" => {
                let mut list: Vec<String> = bus.hosts.keys().cloned().collect();
                list.sort();
                bus.send(&me, &format!("HOSTS {}", list.join(" ")));
            }
            "CONNECT" => match bus.find(argument.trim()) {
                None => bus.send(&me, "ERR host not found, use ip:port from hosts"),
                Some(target) if target == me => {
                    bus.send(&me, "ERR you cannot connect to yourself")
                }
                Some(target) if bus.busy(&target) || bus.busy(&me) => {
                    bus.send(&me, &format!("ERR {} is busy right now", target))
                }
                Some(target) => {
                    bus.requests.insert(target.clone(), me.clone());
                    bus.send(&target, &format!("REQ {}", me));
                    bus.logger
                        .write(&format!("{} sent connection request to {}", me, target));
                }
            },
            "ACCEPT" => {
                if let Some(from) = bus.requests.remove(&me) {
                    bus.peers.insert(me.clone(), from.clone());
                    bus.peers.insert(from.clone(), me.clone());
                    bus.send(&from, &format!("ACCEPTED {}", me));
                    bus.logger
                        .write(&format!("{} accepted the request of {}", me, from));
                }
            }
            "REJECT" => {
                if let Some(from) = bus.requests.remove(&me) {
                    bus.send(&from, &format!("REJECTED {}", me));
                    bus.logger
                        .write(&format!("{} rejected the request of {}", me, from));
                }
            }
            "EXIT" => {
                if let Some(peer) = bus.peers.get(&me).cloned() {
                    bus.send(&peer, &format!("CLOSEREQ {}", me));
                    bus.logger
                        .write(&format!("{} sent close request to {}", me, peer));
                }
            }
            "CLOSEYES" => {
                if let Some(peer) = bus.peers.remove(&me) {
                    bus.peers.remove(&peer);
                    bus.send(&peer, &format!("CLOSED {}", me));
                    bus.logger.write(&format!(
                        "connection between {} and {} closed",
                        peer, me
                    ));
                }
            }
            "CLOSENO" => {
                if let Some(peer) = bus.peers.get(&me).cloned() {
                    bus.send(&peer, &format!("STAY {}", me));
                    bus.logger
                        .write(&format!("{} rejected the close request of {}", me, peer));
                }
            }
            _ => {}
        }
    }
    bus.lock().unwrap().leave(&me);
}

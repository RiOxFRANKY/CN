use netchat::{cdma, walsh};
use socket2::{Domain, Socket, Type};
use std::collections::{BTreeMap, VecDeque};
use std::env;
use std::io::{self, BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::process;
use std::sync::mpsc;
use std::thread;

enum Event {
    Input(String),
    Net(String),
    Closed,
}

struct Session {
    out: TcpStream,
    group: Option<u32>,
    code: Vec<i8>,
    keys: BTreeMap<String, Vec<i8>>,
    pending: VecDeque<String>,
    current: Option<String>,
}

impl Session {
    fn send(&mut self, msg: &str) {
        let _ = writeln!(self.out, "{}", msg);
    }

    fn input(&mut self, line: &str) {
        if self.group.is_some() {
            match line {
                "" => {}
                "leave" => self.send("LEAVE"),
                "keys" => self.show_keys(),
                _ if self.current.is_some() => println!("exchanging Walsh codes, wait"),
                _ => {
                    let signal = cdma::encode_text(line, &self.code);
                    let message = format!(
                        "SIGNAL {} {}",
                        self.code.len(),
                        cdma::format_signal(&signal)
                    );
                    self.send(&message);
                }
            }
            return;
        }

        let mut parts = line.split_whitespace();
        match parts.next() {
            Some("help") => {
                println!("help                     show commands");
                println!("hosts                    show connected hosts");
                println!("groups                   show open groups");
                println!("create                   open a new group chat");
                println!("join <no|owner ip:port>  join a group chat");
            }
            Some("hosts") => self.send("HOSTS"),
            Some("groups") => self.send("GROUPS"),
            Some("create") => self.send("CREATE"),
            Some("join") => match parts.find(|part| *part != "group") {
                Some(target) => self.send(&format!("JOIN {}", target)),
                None => println!("usage: join <group_no|owner_ip:port>"),
            },
            Some(cmd) => println!("unknown command '{}', type help", cmd),
            None => {}
        }
    }

    fn exchange(&mut self) {
        self.current = self.pending.pop_front();
        match self.current.clone() {
            Some(host) => {
                println!("sending my Walsh code to {}", host);
                let message = format!("HELLO {} {}", host, walsh::format(&self.code));
                self.send(&message);
            }
            None => {
                self.send("READY");
                println!("Walsh code exchange finished, chat is full duplex now");
                println!("type a message to send, keys to show codes, leave to exit the group");
                self.show_keys();
            }
        }
    }

    fn show_keys(&self) {
        println!("you {}", walsh::display(&self.code));
        for (host, code) in &self.keys {
            println!("{} {}", host, walsh::display(code));
        }
    }

    fn reset(&mut self) {
        self.group = None;
        self.code.clear();
        self.keys.clear();
        self.pending.clear();
        self.current = None;
    }

    fn net(&mut self, line: &str) {
        let (cmd, rest) = line.split_once(' ').unwrap_or((line, ""));
        let rest = rest.trim();
        match cmd {
            "HOSTS" => {
                for h in rest.split_whitespace() {
                    println!("{}", h);
                }
            }
            "GROUPS" => {
                if rest.is_empty() {
                    println!("no groups, use create to open one");
                }
                for entry in rest.split_whitespace() {
                    let fields: Vec<&str> = entry.split(',').collect();
                    if let [id, owner, count] = fields[..] {
                        println!("group {}  owner {}  members {}", id, owner, count);
                    }
                }
            }
            "JOINED" => {
                let mut parts = rest.split_whitespace();
                let (Some(id), Some(code)) = (
                    parts.next().and_then(|id| id.parse().ok()),
                    parts.next().and_then(walsh::parse),
                ) else {
                    return;
                };
                self.reset();
                self.group = Some(id);
                self.code = code;
                self.pending = parts.map(String::from).collect();
                println!(
                    "\rjoined group {} with Walsh code {}",
                    id,
                    walsh::display(&self.code)
                );
                self.exchange();
            }
            "ORDER" => {
                let Ok(order) = rest.parse::<usize>() else {
                    return;
                };
                if self.group.is_none() {
                    return;
                }
                self.code = walsh::resize(&self.code, order);
                for code in self.keys.values_mut() {
                    *code = walsh::resize(code, order);
                }
                println!(
                    "\rWalsh code length changed to {}, my code is now {}",
                    order,
                    walsh::display(&self.code)
                );
            }
            "HELLO" => {
                let Some((from, code)) = rest.split_once(' ') else {
                    return;
                };
                let Some(code) = walsh::parse(code) else {
                    return;
                };
                let code = walsh::resize(&code, self.code.len());
                println!(
                    "\rreceived Walsh code {} from {}",
                    walsh::display(&code),
                    from
                );
                self.keys.insert(from.to_string(), code);
                let message = format!("REPLY {} {}", from, walsh::format(&self.code));
                self.send(&message);
                println!("sent my Walsh code to {}", from);
            }
            "REPLY" => {
                let Some((from, code)) = rest.split_once(' ') else {
                    return;
                };
                let Some(code) = walsh::parse(code) else {
                    return;
                };
                let code = walsh::resize(&code, self.code.len());
                if self.current.as_deref() == Some(from) {
                    println!(
                        "received Walsh code {} from {}",
                        walsh::display(&code),
                        from
                    );
                    self.keys.insert(from.to_string(), code);
                    self.exchange();
                }
            }
            "SIGNAL" => {
                let Some(signal) = cdma::parse_signal(rest) else {
                    return;
                };
                for (host, code) in &self.keys {
                    if let Ok(text) = cdma::decode_text(&signal, code) {
                        println!("\r{}> {}", host, text);
                        break;
                    }
                }
            }
            "GONE" => {
                self.keys.remove(rest);
                self.pending.retain(|host| host != rest);
                println!("\r{} left the group", rest);
                if self.current.as_deref() == Some(rest) {
                    self.exchange();
                }
            }
            "LEFT" => {
                self.reset();
                println!("left group {}", rest);
            }
            "CLOSED" => {
                self.reset();
                println!("\rgroup {} was closed by its owner", rest);
            }
            "ERR" => println!("\r{}", rest),
            _ => {}
        }
    }

    fn prompt(&self) {
        match self.group {
            Some(id) => print!("group {}> ", id),
            None => print!("> "),
        }
        let _ = io::stdout().flush();
    }
}

fn usage() -> ! {
    eprintln!("usage: sender [-i channel_ip] [-c channel_port] [-p my_port]");
    process::exit(1);
}

fn nearby(port: u16) -> Vec<u16> {
    let mut ports = vec![port];
    for d in 1..=100 {
        ports.extend(port.checked_add(d));
        ports.extend(port.checked_sub(d).filter(|&p| p > 0));
    }
    ports
}

fn open(server: SocketAddr, port: u16) -> io::Result<TcpStream> {
    let ports = if port == 0 { vec![0] } else { nearby(port) };
    for p in ports {
        let sock = Socket::new(Domain::IPV4, Type::STREAM, None)?;
        if sock.bind(&SocketAddr::from(([0, 0, 0, 0], p)).into()).is_err() {
            continue;
        }
        match sock.connect(&server.into()) {
            Ok(()) => return Ok(sock.into()),
            Err(e) if e.kind() == io::ErrorKind::AddrInUse => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(io::ErrorKind::AddrInUse, "no free port found"))
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let mut ip = String::from("127.0.0.1");
    let mut port = 0;
    let mut channel_port = 9000;
    let mut i = 1;
    while i < args.len() {
        let Some(val) = args.get(i + 1) else { usage() };
        match args[i].as_str() {
            "-i" => ip = val.clone(),
            "-p" => port = val.parse().unwrap_or_else(|_| usage()),
            "-c" => channel_port = val.parse().unwrap_or_else(|_| usage()),
            _ => usage(),
        }
        i += 2;
    }

    let server = (ip.as_str(), channel_port)
        .to_socket_addrs()
        .ok()
        .and_then(|mut a| a.find(|a| a.is_ipv4()))
        .unwrap_or_else(|| {
            eprintln!("invalid ip {}", ip);
            process::exit(1);
        });

    let stream = open(server, port).unwrap_or_else(|e| {
        eprintln!("could not connect to channel at {}: {}", server, e);
        process::exit(1);
    });
    let my_port = stream.local_addr().unwrap().port();
    if port != 0 && my_port != port {
        println!("port {} is busy, using {}", port, my_port);
    }

    let password = netchat::password::read_masked("channel password: ").unwrap_or_default();

    let mut out = stream.try_clone().unwrap();
    writeln!(out, "{}", password).unwrap();
    let mut reader = BufReader::new(stream);
    let mut reply = String::new();
    reader.read_line(&mut reply).unwrap_or(0);
    let Some(me) = reply.trim().strip_prefix("OK ").map(String::from) else {
        println!("wrong password, rejected by channel");
        process::exit(1);
    };
    println!(
        "joined the CDMA channel as {}, type help to see commands",
        me
    );

    let (tx, rx) = mpsc::channel();
    let net_tx = tx.clone();
    thread::spawn(move || {
        for line in reader.lines() {
            let Ok(line) = line else { break };
            if net_tx.send(Event::Net(line)).is_err() {
                break;
            }
        }
        let _ = net_tx.send(Event::Closed);
    });
    thread::spawn(move || {
        for line in io::stdin().lines() {
            let Ok(line) = line else { break };
            if tx.send(Event::Input(line)).is_err() {
                break;
            }
        }
        process::exit(0);
    });

    let mut session = Session {
        out,
        group: None,
        code: Vec::new(),
        keys: BTreeMap::new(),
        pending: VecDeque::new(),
        current: None,
    };
    session.prompt();
    for event in rx {
        match event {
            Event::Input(line) => session.input(line.trim()),
            Event::Net(line) => session.net(&line),
            Event::Closed => {
                println!("\nchannel closed");
                break;
            }
        }
        session.prompt();
    }
}

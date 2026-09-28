use socket2::{Domain, Socket, Type};
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
    me: String,
    peer: Option<String>,
    my_turn: bool,
    asked_by: Option<String>,
    close_by: Option<String>,
    waiting: bool,
}

impl Session {
    fn send(&mut self, msg: &str) {
        let _ = writeln!(self.out, "{}", msg);
    }

    fn input(&mut self, line: &str) {
        if let Some(from) = self.asked_by.clone() {
            match answer(line) {
                Some(true) => {
                    self.send("ACCEPT");
                    self.asked_by = None;
                    println!("--- chat with {} (type exit to close) ---", from);
                    println!("waiting for {} to speak first", from);
                    self.peer = Some(from);
                    self.my_turn = false;
                }
                Some(false) => {
                    self.send("REJECT");
                    self.asked_by = None;
                }
                None => println!("press y or n"),
            }
            return;
        }

        if self.close_by.is_some() {
            match answer(line) {
                Some(true) => {
                    self.send("CLOSEYES");
                    self.close_by = None;
                    if let Some(p) = self.peer.take() {
                        println!("--- connection with {} closed ---", p);
                    }
                }
                Some(false) => {
                    self.send("CLOSENO");
                    self.close_by = None;
                }
                None => println!("press y or n"),
            }
            return;
        }

        if self.waiting {
            println!("waiting for response...");
            return;
        }

        if let Some(p) = self.peer.clone() {
            if line == "exit" {
                self.send("EXIT");
                self.waiting = true;
                println!("asked {} to close the connection", p);
            } else if !self.my_turn {
                println!("half duplex: wait for {} to reply", p);
            } else if !line.is_empty() {
                self.send(&format!("MSG {}", line));
                self.my_turn = false;
            }
            return;
        }

        let mut parts = line.split_whitespace();
        match parts.next() {
            Some("help") => {
                println!("help              show commands");
                println!("hosts             show connected hosts");
                println!("connect <host>    send connection request to a host");
            }
            Some("hosts") => {
                self.send("HOSTS");
                self.waiting = true;
            }
            Some("connect") => match parts.next() {
                Some(host) => {
                    self.send(&format!("CONNECT {}", host));
                    self.waiting = true;
                }
                None => println!("usage: connect <host_ip:port>"),
            },
            Some(cmd) => println!("unknown command '{}', type help", cmd),
            None => {}
        }
    }

    fn net(&mut self, line: &str) {
        let (cmd, rest) = line.split_once(' ').unwrap_or((line, ""));
        match cmd {
            "HOSTS" => {
                self.waiting = false;
                for h in rest.split_whitespace() {
                    if h == self.me {
                        println!("{} (you)", h);
                    } else {
                        println!("{}", h);
                    }
                }
            }
            "REQ" => {
                println!("\n{} wants to connect with you", rest);
                self.asked_by = Some(rest.to_string());
            }
            "ACCEPTED" => {
                self.waiting = false;
                self.peer = Some(rest.to_string());
                self.my_turn = true;
                println!("{} accepted your request", rest);
                println!("--- chat with {} (type exit to close) ---", rest);
            }
            "REJECTED" => {
                self.waiting = false;
                println!("{} rejected your request", rest);
            }
            "MSG" => {
                let (from, text) = rest.split_once(' ').unwrap_or((rest, ""));
                println!("{}> {}", from, text);
                self.my_turn = true;
            }
            "CLOSEREQ" => {
                println!("\n{} wants to close the connection", rest);
                self.close_by = Some(rest.to_string());
            }
            "CLOSED" => {
                self.waiting = false;
                self.peer = None;
                println!("{} accepted", rest);
                println!("--- connection with {} closed ---", rest);
            }
            "STAY" => {
                self.waiting = false;
                println!("{} does not want to close the connection", rest);
            }
            "LEFT" => {
                println!("\n{} left the channel", rest);
                self.waiting = false;
                if self.peer.as_deref() == Some(rest) {
                    self.peer = None;
                }
                if self.asked_by.as_deref() == Some(rest) {
                    self.asked_by = None;
                }
                if self.close_by.as_deref() == Some(rest) {
                    self.close_by = None;
                }
            }
            "ERR" => {
                self.waiting = false;
                println!("{}", rest);
            }
            _ => {}
        }
    }

    fn prompt(&self) {
        if self.asked_by.is_some() || self.close_by.is_some() {
            print!("accept? [y/n] ");
        } else if self.waiting || (self.peer.is_some() && !self.my_turn) {
            return;
        } else if self.peer.is_some() {
            print!("you> ");
        } else {
            print!("> ");
        }
        let _ = io::stdout().flush();
    }
}

fn answer(line: &str) -> Option<bool> {
    match line.to_lowercase().as_str() {
        "y" | "yes" => Some(true),
        "n" | "no" => Some(false),
        _ => None,
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

    print!("channel password: ");
    io::stdout().flush().unwrap();
    let mut password = String::new();
    io::stdin().read_line(&mut password).unwrap();

    let mut out = stream.try_clone().unwrap();
    writeln!(out, "{}", password.trim()).unwrap();
    let mut reader = BufReader::new(stream);
    let mut reply = String::new();
    reader.read_line(&mut reply).unwrap_or(0);
    let Some(me) = reply.trim().strip_prefix("OK ").map(String::from) else {
        println!("wrong password, rejected by channel");
        process::exit(1);
    };
    println!("joined the channel as {}, type help to see commands", me);

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
        me,
        peer: None,
        my_turn: false,
        asked_by: None,
        close_by: None,
        waiting: false,
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

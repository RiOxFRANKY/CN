use std::collections::HashMap;
use std::env;
use std::io::{self, BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::process;
use std::sync::{Arc, Mutex};
use std::thread;

#[derive(Default)]
struct Bus {
    hosts: HashMap<String, TcpStream>,
    peers: HashMap<String, String>,
    requests: HashMap<String, String>,
}

impl Bus {
    fn send(&mut self, to: &str, msg: &str) {
        if let Some(s) = self.hosts.get_mut(to) {
            let _ = writeln!(s, "{}", msg);
        }
    }

    fn find(&self, name: &str) -> Option<String> {
        if self.hosts.contains_key(name) {
            return Some(name.to_string());
        }
        let found: Vec<&String> = self
            .hosts
            .keys()
            .filter(|h| h.rsplit_once(':').map(|(ip, _)| ip) == Some(name))
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
            || self.requests.values().any(|v| v == host)
    }

    fn leave(&mut self, me: &str) {
        self.hosts.remove(me);
        let mut others = Vec::new();
        if let Some(p) = self.peers.remove(me) {
            self.peers.remove(&p);
            others.push(p);
        }
        if let Some(p) = self.requests.remove(me) {
            others.push(p);
        }
        let target = self
            .requests
            .iter()
            .find(|(_, v)| v.as_str() == me)
            .map(|(k, _)| k.clone());
        if let Some(t) = target {
            self.requests.remove(&t);
            others.push(t);
        }
        for o in others {
            self.send(&o, &format!("LEFT {}", me));
        }
        println!("{} left the channel", me);
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let mut port = 9000;
    if args.len() == 3 && args[1] == "-p" {
        port = args[2].parse().unwrap_or_else(|_| usage());
    } else if args.len() != 1 {
        usage();
    }

    let listener = bind_near(port);
    println!("channel created on port {}", listener.local_addr().unwrap().port());

    print!("set password: ");
    io::stdout().flush().unwrap();
    let mut password = String::new();
    io::stdin().read_line(&mut password).unwrap();
    let password = password.trim().to_string();

    println!("waiting for senders...");
    let bus = Arc::new(Mutex::new(Bus::default()));
    for stream in listener.incoming().flatten() {
        let bus = bus.clone();
        let password = password.clone();
        thread::spawn(move || handle(stream, bus, password));
    }
}

fn usage() -> ! {
    eprintln!("usage: channel [-p port]");
    process::exit(1);
}

fn bind_near(port: u16) -> TcpListener {
    for p in nearby(port) {
        if let Ok(l) = TcpListener::bind(("0.0.0.0", p)) {
            if p != port {
                println!("port {} is busy, using {}", port, p);
            }
            return l;
        }
    }
    eprintln!("no free port near {}", port);
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

fn handle(stream: TcpStream, bus: Arc<Mutex<Bus>>, password: String) {
    let Ok(addr) = stream.peer_addr() else { return };
    let me = addr.to_string();
    let Ok(mut out) = stream.try_clone() else { return };
    let mut lines = BufReader::new(stream).lines();

    let given = lines.next().and_then(|l| l.ok()).unwrap_or_default();
    if given.trim() != password {
        let _ = writeln!(out, "WRONG");
        println!("{} entered wrong password, rejected", me);
        return;
    }
    let _ = writeln!(out, "OK {}", me);
    bus.lock().unwrap().hosts.insert(me.clone(), out);
    println!("{} joined the channel", me);

    for line in lines {
        let Ok(line) = line else { break };
        let (cmd, arg) = line.split_once(' ').unwrap_or((&line, ""));
        let mut bus = bus.lock().unwrap();
        match cmd {
            "HOSTS" => {
                let mut list: Vec<String> = bus.hosts.keys().cloned().collect();
                list.sort();
                bus.send(&me, &format!("HOSTS {}", list.join(" ")));
            }
            "CONNECT" => match bus.find(arg.trim()) {
                None => bus.send(&me, "ERR host not found, use ip:port from hosts"),
                Some(t) if t == me => bus.send(&me, "ERR you cannot connect to yourself"),
                Some(t) if bus.busy(&t) || bus.busy(&me) => {
                    bus.send(&me, &format!("ERR {} is busy right now", t))
                }
                Some(t) => {
                    bus.requests.insert(t.clone(), me.clone());
                    bus.send(&t, &format!("REQ {}", me));
                    println!("{} sent connection request to {}", me, t);
                }
            },
            "ACCEPT" => {
                if let Some(from) = bus.requests.remove(&me) {
                    bus.peers.insert(me.clone(), from.clone());
                    bus.peers.insert(from.clone(), me.clone());
                    bus.send(&from, &format!("ACCEPTED {}", me));
                    println!("{} accepted the request of {}", me, from);
                }
            }
            "REJECT" => {
                if let Some(from) = bus.requests.remove(&me) {
                    bus.send(&from, &format!("REJECTED {}", me));
                    println!("{} rejected the request of {}", me, from);
                }
            }
            "MSG" => {
                if let Some(p) = bus.peers.get(&me).cloned() {
                    bus.send(&p, &format!("MSG {} {}", me, arg));
                    println!("{} sent message to {}", me, p);
                }
            }
            "EXIT" => {
                if let Some(p) = bus.peers.get(&me).cloned() {
                    bus.send(&p, &format!("CLOSEREQ {}", me));
                    println!("{} sent close request to {}", me, p);
                }
            }
            "CLOSEYES" => {
                if let Some(p) = bus.peers.remove(&me) {
                    bus.peers.remove(&p);
                    bus.send(&p, &format!("CLOSED {}", me));
                    println!("{} accepted the close request of {}", me, p);
                    println!("connection between {} and {} closed", p, me);
                }
            }
            "CLOSENO" => {
                if let Some(p) = bus.peers.get(&me).cloned() {
                    bus.send(&p, &format!("STAY {}", me));
                    println!("{} rejected the close request of {}", me, p);
                }
            }
            _ => {}
        }
    }
    bus.lock().unwrap().leave(&me);
}

extern crate octobuild;
extern crate daemon;
extern crate router;
extern crate fern;
extern crate hyper;
extern crate rustc_serialize;
#[macro_use]
extern crate log;

use octobuild::cluster::common::{BuilderInfo, BuilderInfoUpdate};
use daemon::State;
use daemon::Daemon;
use daemon::DaemonRunner;
use hyper::Client;
use rustc_serialize::json;
use std::error::Error;
use std::io;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc::Receiver;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc};
use std::str::FromStr;
use std::time::Duration;
use std::thread;
use std::thread::JoinHandle;

struct AgentService {
    done: Arc<AtomicBool>,
    listener: Option<TcpListener>,
    accepter: Option<JoinHandle<()>>,
    anoncer: Option<JoinHandle<()>>,
}

impl AgentService {
    fn new() -> AgentService {
        let addr: SocketAddr = FromStr::from_str("127.0.0.1:0").ok().expect("Failed to parse host:port string");
        let listener = TcpListener::bind(&addr).ok().expect("Failed to bind address");

        let endpoint = listener.local_addr().unwrap().to_string();
        let info = BuilderInfoUpdate::new(BuilderInfo {
            name: get_name(),
            endpoints: vec!(endpoint),
        });
        let done = Arc::new(AtomicBool::new(false));
        AgentService {
            accepter: Some(AgentService::thread_accepter(listener.try_clone().unwrap())),
            anoncer: Some(AgentService::thread_anoncer(info, done.clone())),
            done: done,
            listener: Some(listener),
        }
    }

    fn thread_accepter(listener: TcpListener) -> JoinHandle<()> {
        thread::spawn(move || {
            // accept connections and process them, spawning a new thread for each one
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => {
                        thread::spawn(move || {
                            // connection succeeded
                            AgentService::handle_client(stream)
                        });
                    }
                    Err(e) => { /* connection failed */ }
                }
            }
        })
    }

    fn thread_anoncer(info: BuilderInfoUpdate, done: Arc<AtomicBool>) -> JoinHandle<()> {
        thread::spawn(move || {
            let client = Client::new();
            while !done.load(Ordering::Relaxed) {
                match client
                .post("http://localhost:3000/rpc/v1/agent/update")
                .body(&json::encode(&info).unwrap())
                .send()
                {
                    Ok(_) => {}
                    Err(e) => {
                        info!("Agent: can't send info to coordinator: {}", e.description());
                    }
                }
                thread::sleep(Duration::from_secs(1));
            }
        })
    }

    fn handle_client(mut stream: TcpStream) -> io::Result<()> {
        try!(stream.write("Hello!!!\n".as_bytes()));
        try!(stream.flush());
        Ok(())
    }
}

impl Drop for AgentService {
    fn drop(&mut self) {
        println!("drop begin");
        self.done.store(true, Ordering::Relaxed);
        self.listener.take();

        match self.anoncer.take() {
            Some(t) => { t.join().unwrap(); },
            None => {},
        }
        match self.accepter.take() {
            Some(t) => { t.join().unwrap(); },
            None => {},
        }
        println!("drop end");
    }
}

fn get_name() -> String {
    octobuild::hostname::get_host_name().unwrap()
}

fn main() {
    let daemon = Daemon {
        name: "octobuild_agent".to_string()
    };

    daemon.run(move |rx: Receiver<State>| {
        octobuild::utils::init_logger();

        info!("Agent started.");
        let mut agent = None;
        for signal in rx.iter() {
            match signal {
                State::Start => {
                    info!("Agent: Starting");
                    agent = Some(AgentService::new());
                    info!("Agent: Readly");
                },
                State::Reload => {
                    info!("Agent: Reload");
                }
                State::Stop => {
                    info!("Agent: Stoping");
                    agent.take();
                    info!("Agent: Stoped");
                }
            };
        }
        info!("Agent shutdowned.");
    }).unwrap();
}

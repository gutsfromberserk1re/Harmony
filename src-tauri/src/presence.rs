//! Discord Rich Presence ("Listening to Harmony").
//!
//! Talks to the Discord client over its local named pipe on a dedicated thread, so Discord being
//! closed, slow or rate-limiting can never freeze the player. No extra crates needed.

use serde_json::{json, Map, Value};
use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    sync::{
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
        Mutex,
    },
    thread,
    time::{Duration, Instant},
};

const MIN_INTERVAL: Duration = Duration::from_secs(4); // Discord allows ~5 updates per 20 s
const RECONNECT: Duration = Duration::from_secs(15); // retry cadence while Discord isn't running

enum Msg {
    Set(Option<Value>),
    Stop,
}

pub struct Presence {
    tx: Mutex<Option<Sender<Msg>>>,
}

impl Presence {
    /// An empty `client_id` disables the feature entirely.
    pub fn new(client_id: &str) -> Self {
        let client_id = client_id.trim().to_string();
        if client_id.is_empty() {
            return Self {
                tx: Mutex::new(None),
            };
        }
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || run(&client_id, rx));
        Self {
            tx: Mutex::new(Some(tx)),
        }
    }

    pub fn enabled(&self) -> bool {
        self.tx.lock().unwrap().is_some()
    }

    /// `None` clears the status.
    pub fn set(&self, activity: Option<Value>) {
        if let Some(tx) = self.tx.lock().unwrap().as_ref() {
            let _ = tx.send(Msg::Set(activity));
        }
    }

    pub fn stop(&self) {
        if let Some(tx) = self.tx.lock().unwrap().as_ref() {
            let _ = tx.send(Msg::Stop);
        }
    }
}

// ----------------------------------------------------------------- IPC

struct Ipc {
    pipe: File,
    nonce: u64,
}

impl Ipc {
    fn connect(client_id: &str) -> io::Result<Self> {
        for i in 0..10 {
            let path = format!(r"\\.\pipe\discord-ipc-{i}");
            if let Ok(pipe) = OpenOptions::new().read(true).write(true).open(path) {
                let mut ipc = Ipc { pipe, nonce: 0 };
                ipc.send(0, &json!({ "v": 1, "client_id": client_id }))?;
                ipc.recv()?; // READY
                return Ok(ipc);
            }
        }
        Err(io::Error::new(io::ErrorKind::NotFound, "Discord is not running"))
    }

    fn send(&mut self, opcode: u32, payload: &Value) -> io::Result<()> {
        let body = payload.to_string();
        let mut frame = Vec::with_capacity(8 + body.len());
        frame.extend_from_slice(&opcode.to_le_bytes());
        frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
        frame.extend_from_slice(body.as_bytes());
        self.pipe.write_all(&frame)?;
        self.pipe.flush()
    }

    fn recv(&mut self) -> io::Result<Value> {
        let mut header = [0u8; 8];
        self.pipe.read_exact(&mut header)?;
        let len = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
        let mut body = vec![0u8; len];
        self.pipe.read_exact(&mut body)?;
        Ok(serde_json::from_slice(&body).unwrap_or(Value::Null))
    }

    fn set_activity(&mut self, activity: Option<&Value>) -> io::Result<()> {
        self.nonce += 1;
        self.send(
            1,
            &json!({
                "cmd": "SET_ACTIVITY",
                "args": { "pid": std::process::id(), "activity": activity },
                "nonce": self.nonce.to_string(),
            }),
        )?;
        self.recv().map(|_| ())
    }
}

// ----------------------------------------------------------------- activity

/// Turns the flat object sent by the UI into Discord's activity payload.
fn build_activity(a: &Value) -> Value {
    let text = |k: &str| a.get(k).and_then(Value::as_str);
    let mut act = Map::new();
    act.insert("type".into(), json!(2)); // Listening

    for key in ["details", "state"] {
        if let Some(v) = text(key) {
            act.insert(key.into(), json!(v));
        }
    }

    let mut times = Map::new();
    for key in ["start", "end"] {
        if let Some(v) = a.get(key).and_then(Value::as_i64) {
            times.insert(key.into(), json!(v));
        }
    }
    if !times.is_empty() {
        act.insert("timestamps".into(), Value::Object(times));
    }

    let mut assets = Map::new();
    for key in ["large_image", "large_text", "small_image", "small_text"] {
        if let Some(v) = text(key) {
            assets.insert(key.into(), json!(v));
        }
    }
    if !assets.is_empty() {
        act.insert("assets".into(), Value::Object(assets));
    }

    if let Some(buttons) = a.get("buttons").filter(|b| b.is_array()) {
        act.insert("buttons".into(), buttons.clone());
    }
    Value::Object(act)
}

/// Same activity apart from timestamps that drift by a few seconds.
fn similar(a: &Option<Value>, b: &Option<Value>) -> bool {
    let (Some(a), Some(b)) = (a, b) else {
        return a.is_none() && b.is_none();
    };
    let core = |v: &Value| {
        let mut v = v.clone();
        if let Some(o) = v.as_object_mut() {
            o.remove("start");
            o.remove("end");
        }
        v
    };
    if core(a) != core(b) {
        return false;
    }
    ["start", "end"].iter().all(|k| {
        match (a.get(*k).and_then(Value::as_i64), b.get(*k).and_then(Value::as_i64)) {
            (None, None) => true,
            (Some(x), Some(y)) => (x - y).abs() <= 3,
            _ => false,
        }
    })
}

// ----------------------------------------------------------------- worker

fn run(client_id: &str, rx: Receiver<Msg>) {
    let mut want: Option<Value> = None;
    let mut sent: Option<Value> = None;
    let mut ipc: Option<Ipc> = None;
    let mut last_send: Option<Instant> = None;

    // Drains queued messages, keeping only the newest state. Returns false on Stop.
    let drain = |want: &mut Option<Value>| -> bool {
        while let Ok(msg) = rx.try_recv() {
            match msg {
                Msg::Set(a) => *want = a,
                Msg::Stop => return false,
            }
        }
        true
    };

    'main: loop {
        match rx.recv_timeout(RECONNECT) {
            Ok(Msg::Set(a)) => want = a,
            Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
        if !drain(&mut want) {
            break;
        }
        if similar(&want, &sent) {
            continue;
        }
        if let Some(gap) = last_send.map(|t| MIN_INTERVAL.saturating_sub(t.elapsed())) {
            if !gap.is_zero() {
                thread::sleep(gap);
                if !drain(&mut want) {
                    break 'main;
                }
            }
        }

        let result = (|| -> io::Result<()> {
            if ipc.is_none() {
                ipc = Some(Ipc::connect(client_id)?);
            }
            let activity = want.as_ref().map(build_activity);
            ipc.as_mut().unwrap().set_activity(activity.as_ref())
        })();

        match result {
            Ok(()) => {
                sent = want.clone();
                last_send = Some(Instant::now());
            }
            Err(_) => {
                ipc = None; // Discord closed / pipe broke: full re-send after reconnecting
                sent = None;
            }
        }
    }

    if let Some(mut ipc) = ipc {
        let _ = ipc.set_activity(None);
    }
}

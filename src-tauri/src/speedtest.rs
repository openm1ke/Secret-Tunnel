//! Speed tests of the running tunnel through its local SOCKS proxy: latency
//! on an open connection, UDP round trips (DNS), single- and multi-stream
//! downloads, uploads, bursts of short requests and a mixed load that looks
//! like several programs at once. Transfers use curl, like the IP lookup;
//! Cloudflare's speed endpoints answer from a nearby edge of the server.
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::net::{TcpStream, UdpSocket};
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const DOWN: &str = "https://speed.cloudflare.com/__down?bytes=";
const UP: &str = "https://speed.cloudflare.com/__up";
/// Download requests stay well under the endpoint's 100 MB limit; it also
/// rate-limits an address that asks for many large ones (HTTP 429).
const CHUNK: u64 = 25_000_000;
/// Used when Cloudflare refuses downloads: a large file near the servers.
const FALLBACK_DOWN: &str = "https://nbg1-speed.hetzner.com/1GB.bin";
const MAX_RUNS: usize = 200;
/// Rates are measured after connections have ramped up.
const WARMUP: Duration = Duration::from_millis(1500);

#[cfg(windows)]
const NULL: &str = "NUL";
#[cfg(not(windows))]
const NULL: &str = "/dev/null";

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct Results {
    pub ping_ms: Option<f64>,
    pub ping_min_ms: Option<f64>,
    pub jitter_ms: Option<f64>,
    pub loaded_ping_ms: Option<f64>,
    pub dns_ms: Option<f64>,
    pub dns_loss: Option<f64>,
    pub download_single_mbps: Option<f64>,
    pub download_mbps: Option<f64>,
    pub upload_mbps: Option<f64>,
    pub requests_per_second: Option<f64>,
    pub request_ms: Option<f64>,
    pub mixed_download_mbps: Option<f64>,
    pub mixed_request_ms: Option<f64>,
    /// Where downloads came from, when not Cloudflare.
    pub download_source: Option<String>,
    pub bytes_down: u64,
    pub bytes_up: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Run {
    pub id: String,
    pub kind: String,
    pub started_at: u64,
    pub finished_at: u64,
    pub server: String,
    pub route: Vec<String>,
    pub country: String,
    pub results: Results,
    pub error: String,
    pub cancelled: bool,
}

/// What the tab shows while a test runs.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Live {
    pub running: bool,
    pub kind: String,
    pub steps: Vec<String>,
    pub step: usize,
    /// Progress of the current step, 0..1.
    pub progress: f64,
    pub down_mbps: f64,
    pub up_mbps: f64,
    pub ping_ms: Option<f64>,
    /// Seconds since start, download and upload Mbit/s.
    pub timeline: Vec<(f64, f64, f64)>,
    pub results: Results,
    pub last: Option<Run>,
    pub error: String,
}

#[derive(Clone, Debug, PartialEq)]
enum Step {
    Ping(usize),
    Dns(usize),
    Download { streams: usize, seconds: u64, loaded_ping: bool },
    Upload { streams: usize, seconds: u64 },
    Requests { count: usize, parallel: usize, bytes: u64 },
    Mixed { seconds: u64 },
}

impl Step {
    fn title(&self) -> String {
        match self {
            Step::Ping(_) => "Пинг".into(),
            Step::Dns(_) => "UDP: DNS-запросы".into(),
            Step::Download { streams: 1, .. } => "Загрузка, 1 поток".into(),
            Step::Download { streams, .. } => format!("Загрузка, {streams} потоков"),
            Step::Upload { streams: 1, .. } => "Отдача, 1 поток".into(),
            Step::Upload { streams, .. } => format!("Отдача, {streams} потока"),
            Step::Requests { count, .. } => format!("{count} коротких запросов"),
            Step::Mixed { .. } => "Смешанная нагрузка".into(),
        }
    }
}

fn plan(kind: &str) -> Option<Vec<Step>> {
    use Step::*;
    Some(match kind {
        "quick" => vec![Ping(10), Download { streams: 1, seconds: 6, loaded_ping: false }, Upload { streams: 1, seconds: 5 }],
        "full" => vec![
            Ping(20),
            Dns(20),
            Download { streams: 1, seconds: 8, loaded_ping: false },
            Download { streams: 6, seconds: 8, loaded_ping: true },
            Upload { streams: 3, seconds: 8 },
            Requests { count: 40, parallel: 6, bytes: 50_000 },
        ],
        "bandwidth" => vec![Ping(10), Download { streams: 8, seconds: 10, loaded_ping: true }, Upload { streams: 4, seconds: 8 }],
        "browsing" => vec![Ping(10), Dns(20), Requests { count: 60, parallel: 6, bytes: 50_000 }],
        "mixed" => vec![Ping(10), Mixed { seconds: 12 }],
        "latency" => vec![Ping(30), Dns(30)],
        _ => return None,
    })
}

pub struct Target {
    pub host: String,
    pub port: u16,
    pub server: String,
    pub route: Vec<String>,
    pub country: String,
}

#[derive(Default)]
pub struct SpeedTests {
    path: Option<PathBuf>,
    history: Vec<Run>,
    live: Arc<Mutex<Live>>,
    cancel: Arc<AtomicBool>,
}

impl SpeedTests {
    pub fn load(path: PathBuf) -> Self {
        let history = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self { path: Some(path), history, ..Default::default() }
    }

    pub fn status(&self) -> Live {
        self.live.lock().map(|live| live.clone()).unwrap_or_default()
    }

    pub fn history(&self) -> Vec<Run> {
        self.history.clone()
    }

    pub fn clear(&mut self) {
        self.history.clear();
        self.save();
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    fn save(&self) {
        let Some(path) = &self.path else { return };
        if let Ok(bytes) = serde_json::to_vec(&self.history) {
            let temporary = path.with_extension("json.tmp");
            if std::fs::write(&temporary, bytes).is_ok() {
                let _ = std::fs::rename(&temporary, path);
            }
        }
    }

    fn record(&mut self, run: Run) {
        self.history.push(run);
        if self.history.len() > MAX_RUNS {
            let overflow = self.history.len() - MAX_RUNS;
            self.history.drain(..overflow);
        }
        self.save();
    }

    /// Start a test in the background. `done` stores the finished run.
    pub fn start(&mut self, kind: &str, target: Target, done: impl FnOnce(Run) + Send + 'static) -> Result<(), String> {
        let steps = plan(kind).ok_or("Неизвестный тест")?;
        if self.live.lock().map(|live| live.running).unwrap_or(false) {
            return Err("Тест уже идёт".into());
        }
        self.cancel.store(false, Ordering::SeqCst);
        let last = self.history.last().cloned();
        if let Ok(mut live) = self.live.lock() {
            *live = Live {
                running: true,
                kind: kind.into(),
                steps: steps.iter().map(Step::title).collect(),
                last,
                ..Default::default()
            };
        }
        let (live, cancel, kind) = (self.live.clone(), self.cancel.clone(), kind.to_owned());
        std::thread::spawn(move || {
            let started = crate::now_epoch();
            let mut run = Run {
                id: format!("{started}-{kind}"),
                kind,
                started_at: started,
                server: target.server.clone(),
                route: target.route.clone(),
                country: target.country.clone(),
                ..Default::default()
            };
            let tester = Tester { proxy: format!("{}:{}", target.host, target.port), host: target.host.clone(), port: target.port, live: live.clone(), cancel: cancel.clone(), started: Instant::now(), refused: Arc::default() };
            for (index, step) in steps.iter().enumerate() {
                if cancel.load(Ordering::SeqCst) {
                    run.cancelled = true;
                    break;
                }
                tester.update(|live| {
                    live.step = index;
                    live.progress = 0.0;
                });
                if let Err(error) = tester.run(step) {
                    run.error = format!("{}: {error}", step.title());
                    break;
                }
            }
            run.cancelled |= cancel.load(Ordering::SeqCst);
            run.finished_at = crate::now_epoch();
            run.results = live.lock().map(|live| live.results.clone()).unwrap_or_default();
            if let Ok(mut live) = live.lock() {
                live.running = false;
                live.down_mbps = 0.0;
                live.up_mbps = 0.0;
                live.error = run.error.clone();
                live.last = Some(run.clone());
            }
            done(run);
        });
        Ok(())
    }

    /// Keep a finished run, unless nothing was measured.
    pub fn finish(&mut self, run: Run) {
        if run.results != Results::default() {
            self.record(run);
        }
    }
}

struct Tester {
    proxy: String,
    host: String,
    port: u16,
    live: Arc<Mutex<Live>>,
    cancel: Arc<AtomicBool>,
    started: Instant,
    /// Cloudflare refused downloads; streams use the fallback.
    refused: Arc<AtomicBool>,
}

fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.total_cmp(b));
    let middle = values.len() / 2;
    Some(if values.len() % 2 == 0 { (values[middle - 1] + values[middle]) / 2.0 } else { values[middle] })
}

/// Mean difference between consecutive samples.
fn jitter(samples: &[f64]) -> Option<f64> {
    (samples.len() > 1).then(|| samples.windows(2).map(|w| (w[1] - w[0]).abs()).sum::<f64>() / (samples.len() - 1) as f64)
}

fn mbps(bytes: u64, elapsed: Duration) -> f64 {
    let seconds = elapsed.as_secs_f64();
    if seconds <= 0.0 { 0.0 } else { bytes as f64 * 8.0 / seconds / 1_000_000.0 }
}

/// Children killed when a step ends.
#[derive(Default)]
struct Pack(Vec<Child>);

impl Drop for Pack {
    fn drop(&mut self) {
        for child in &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Tester {
    fn update(&self, change: impl FnOnce(&mut Live)) {
        if let Ok(mut live) = self.live.lock() {
            change(&mut live);
        }
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    fn curl(&self) -> std::process::Command {
        let mut command = crate::background_command("curl");
        command.args(["-s", "--socks5-hostname", &self.proxy, "--connect-timeout", "10"]);
        command.stdin(Stdio::null()).stderr(Stdio::null());
        command
    }

    fn run(&self, step: &Step) -> Result<(), String> {
        match step {
            Step::Ping(count) => self.ping(*count),
            Step::Dns(count) => self.dns(*count),
            Step::Download { streams, seconds, loaded_ping } => self.download(*streams, *seconds, *loaded_ping),
            Step::Upload { streams, seconds } => self.upload(*streams, *seconds),
            Step::Requests { count, parallel, bytes } => self.requests(*count, *parallel, *bytes),
            Step::Mixed { seconds } => self.mixed(*seconds),
        }
    }

    /// A curl that prints the time to first byte of `count` empty requests
    /// on one connection: after the first (TLS setup) each is a round trip.
    /// Timings go to stderr, which is unbuffered, so they arrive as measured
    /// and are not lost when the process is stopped.
    fn ping_process(&self, count: usize) -> Result<Child, String> {
        let mut command = self.curl();
        command.args(["-w", "%{stderr}%{time_starttransfer}\\n"]);
        for _ in 0..=count {
            command.args(["-o", NULL, &format!("{DOWN}0")]);
        }
        command.stdout(Stdio::null()).stderr(Stdio::piped()).spawn().map_err(|e| format!("curl: {e}"))
    }

    /// Round trips read from a ping process, the first one skipped.
    fn ping_samples(child: &mut Child, samples: Arc<Mutex<Vec<f64>>>, live: Arc<Mutex<Live>>) -> std::thread::JoinHandle<()> {
        let stderr = child.stderr.take();
        std::thread::spawn(move || {
            let Some(stderr) = stderr else { return };
            let reader = std::io::BufReader::new(stderr);
            for (index, line) in std::io::BufRead::lines(reader).map_while(Result::ok).enumerate() {
                let Ok(seconds) = line.trim().parse::<f64>() else { continue };
                if index == 0 || seconds <= 0.0 {
                    continue;
                }
                let ms = seconds * 1000.0;
                if let Ok(mut samples) = samples.lock() {
                    samples.push(ms);
                }
                if let Ok(mut live) = live.lock() {
                    live.ping_ms = Some(ms);
                }
            }
        })
    }

    fn ping(&self, count: usize) -> Result<(), String> {
        let mut pack = Pack::default();
        let mut child = self.ping_process(count)?;
        let samples: Arc<Mutex<Vec<f64>>> = Arc::default();
        let reader = Self::ping_samples(&mut child, samples.clone(), self.live.clone());
        pack.0.push(child);
        let deadline = Instant::now() + Duration::from_secs(10 + count as u64);
        loop {
            let done = samples.lock().map(|s| s.len()).unwrap_or(0);
            self.update(|live| live.progress = done as f64 / count as f64);
            if done >= count || self.cancelled() || Instant::now() > deadline || pack.0[0].try_wait().ok().flatten().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        drop(pack);
        let _ = reader.join();
        let mut samples = samples.lock().map(|s| s.clone()).unwrap_or_default();
        if samples.is_empty() {
            return Err("нет ответа через туннель".into());
        }
        let jitter = jitter(&samples);
        let min = samples.iter().copied().fold(f64::INFINITY, f64::min);
        let ping = median(&mut samples);
        self.update(|live| {
            live.results.ping_ms = ping;
            live.results.ping_min_ms = Some(min);
            live.results.jitter_ms = jitter;
            live.progress = 1.0;
        });
        Ok(())
    }

    /// DNS questions over the tunnel's UDP relay (SOCKS5 UDP ASSOCIATE).
    fn dns(&self, count: usize) -> Result<(), String> {
        let mut control = TcpStream::connect((self.host.as_str(), self.port)).map_err(|e| e.to_string())?;
        control.set_read_timeout(Some(Duration::from_secs(5))).ok();
        control.write_all(&[5, 1, 0]).map_err(|e| e.to_string())?;
        let mut reply = [0_u8; 2];
        control.read_exact(&mut reply).map_err(|e| e.to_string())?;
        control.write_all(&[5, 3, 0, 1, 0, 0, 0, 0, 0, 0]).map_err(|e| e.to_string())?;
        let mut head = [0_u8; 4];
        control.read_exact(&mut head).map_err(|e| e.to_string())?;
        if head[1] != 0 {
            return Err(format!("туннель не открыл UDP (код {})", head[1]));
        }
        let relay_ip: std::net::IpAddr = match head[3] {
            1 => {
                let mut ip = [0_u8; 4];
                control.read_exact(&mut ip).map_err(|e| e.to_string())?;
                ip.into()
            }
            4 => {
                let mut ip = [0_u8; 16];
                control.read_exact(&mut ip).map_err(|e| e.to_string())?;
                ip.into()
            }
            _ => return Err("неожиданный ответ SOCKS".into()),
        };
        let mut port = [0_u8; 2];
        control.read_exact(&mut port).map_err(|e| e.to_string())?;
        let relay = std::net::SocketAddr::new(
            if relay_ip.is_unspecified() { self.host.parse().unwrap_or(relay_ip) } else { relay_ip },
            u16::from_be_bytes(port),
        );
        let socket = UdpSocket::bind((if relay.is_ipv4() { "127.0.0.1" } else { "::1" }, 0)).map_err(|e| e.to_string())?;
        socket.set_read_timeout(Some(Duration::from_millis(1500))).ok();
        let mut samples = Vec::new();
        let mut lost = 0;
        for index in 0..count {
            if self.cancelled() {
                break;
            }
            let id = (index as u16).wrapping_add(0x5100);
            let mut packet = vec![0, 0, 0, 1, 1, 1, 1, 1, 0, 53];
            packet.extend_from_slice(&id.to_be_bytes());
            packet.extend_from_slice(&[1, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
            for label in ["cloudflare", "com"] {
                packet.push(label.len() as u8);
                packet.extend_from_slice(label.as_bytes());
            }
            packet.extend_from_slice(&[0, 0, 1, 0, 1]);
            let sent = Instant::now();
            socket.send_to(&packet, relay).map_err(|e| e.to_string())?;
            let mut buffer = [0_u8; 1500];
            let answered = loop {
                match socket.recv_from(&mut buffer) {
                    Ok((size, _)) if size > 12 && buffer[10..12] == id.to_be_bytes() => break true,
                    Ok(_) => continue,
                    Err(_) => break false,
                }
            };
            if answered {
                let ms = sent.elapsed().as_secs_f64() * 1000.0;
                samples.push(ms);
                self.update(|live| live.ping_ms = Some(ms));
            } else {
                lost += 1;
            }
            self.update(|live| live.progress = (index + 1) as f64 / count as f64);
            std::thread::sleep(Duration::from_millis(100));
        }
        drop(control);
        let asked = samples.len() + lost;
        let dns = median(&mut samples);
        self.update(|live| {
            live.results.dns_ms = dns;
            live.results.dns_loss = (asked > 0).then(|| lost as f64 / asked as f64);
        });
        if dns.is_none() && asked > 0 {
            return Err("UDP-запросы не получили ответа".into());
        }
        Ok(())
    }

    /// Long downloads, each a chain of the largest allowed requests on one
    /// connection, counted as the bytes arrive.
    fn download_stream(&self, counter: Arc<AtomicU64>) -> Result<Child, String> {
        let mut command = self.curl();
        if self.refused.load(Ordering::Relaxed) {
            command.arg(FALLBACK_DOWN);
        } else {
            command.args(["-w", "%{stderr}%{http_code}\\n"]);
            for _ in 0..40 {
                command.arg(format!("{DOWN}{CHUNK}"));
            }
            command.stderr(Stdio::piped());
        }
        let mut child = command.stdout(Stdio::piped()).spawn().map_err(|e| format!("curl: {e}"))?;
        if let Some(stderr) = child.stderr.take() {
            let refused = self.refused.clone();
            std::thread::spawn(move || {
                for line in std::io::BufRead::lines(std::io::BufReader::new(stderr)).map_while(Result::ok) {
                    if matches!(line.trim(), "429" | "403") {
                        refused.store(true, Ordering::Relaxed);
                    }
                }
            });
        }
        if let Some(mut stdout) = child.stdout.take() {
            std::thread::spawn(move || {
                let mut buffer = vec![0_u8; 64 * 1024];
                while let Ok(size) = stdout.read(&mut buffer) {
                    if size == 0 {
                        break;
                    }
                    counter.fetch_add(size as u64, Ordering::Relaxed);
                }
            });
        }
        Ok(child)
    }

    /// An upload that streams from our pipe for as long as the step runs.
    fn upload_stream(&self, counter: Arc<AtomicU64>, stop: Arc<AtomicBool>) -> Result<Child, String> {
        let mut command = self.curl();
        command.args(["-X", "POST", "-T", "-", "-o", NULL, UP]);
        let mut child = command.stdin(Stdio::piped()).stdout(Stdio::null()).spawn().map_err(|e| format!("curl: {e}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            std::thread::spawn(move || {
                // Incompressible bytes, so no layer can shrink them.
                let mut state = 0x9e37_79b9_7f4a_7c15_u64;
                let buffer: Vec<u8> = (0..64 * 1024)
                    .map(|_| {
                        state ^= state << 13;
                        state ^= state >> 7;
                        state ^= state << 17;
                        state as u8
                    })
                    .collect();
                while !stop.load(Ordering::Relaxed) {
                    if stdin.write_all(&buffer).is_err() {
                        break;
                    }
                    counter.fetch_add(buffer.len() as u64, Ordering::Relaxed);
                }
            });
        }
        Ok(child)
    }

    /// Run streams for a while, restarting any that end, and report rates.
    /// Returns the bytes counted after the warm-up and the time they took.
    fn sustain(
        &self,
        seconds: u64,
        streams: usize,
        upload: bool,
        mut extra: impl FnMut(&Self, f64),
    ) -> Result<(u64, Duration, u64), String> {
        let counter: Arc<AtomicU64> = Arc::default();
        let stop: Arc<AtomicBool> = Arc::default();
        let mut pack = Pack::default();
        let start_one = |counter: &Arc<AtomicU64>| if upload { self.upload_stream(counter.clone(), stop.clone()) } else { self.download_stream(counter.clone()) };
        for _ in 0..streams {
            pack.0.push(start_one(&counter)?);
        }
        let start = Instant::now();
        let total = Duration::from_secs(seconds);
        let mut window: std::collections::VecDeque<(Instant, u64)> = Default::default();
        let mut after_warmup: Option<(Instant, u64)> = None;
        while start.elapsed() < total && !self.cancelled() {
            std::thread::sleep(Duration::from_millis(250));
            for child in &mut pack.0 {
                if child.try_wait().ok().flatten().is_some() {
                    if let Ok(fresh) = start_one(&counter) {
                        let mut old = std::mem::replace(child, fresh);
                        let _ = old.wait();
                    }
                }
            }
            let now = Instant::now();
            let bytes = counter.load(Ordering::Relaxed);
            window.push_back((now, bytes));
            while window.front().is_some_and(|(at, _)| now.duration_since(*at) > Duration::from_secs(1)) {
                window.pop_front();
            }
            let rate = window.front().map(|(at, first)| mbps(bytes - first, now.duration_since(*at))).unwrap_or(0.0);
            if after_warmup.is_none() && start.elapsed() >= WARMUP {
                after_warmup = Some((now, bytes));
            }
            let progress = start.elapsed().as_secs_f64() / total.as_secs_f64();
            let elapsed = self.started.elapsed().as_secs_f64();
            self.update(|live| {
                if upload { live.up_mbps = rate } else { live.down_mbps = rate }
                live.progress = progress.min(1.0);
                let (down, up) = (live.down_mbps, live.up_mbps);
                live.timeline.push((elapsed, if upload { 0.0 } else { down }, if upload { up } else { 0.0 }));
                if live.timeline.len() > 2400 {
                    live.timeline.drain(..600);
                }
            });
            extra(self, progress);
        }
        stop.store(true, Ordering::Relaxed);
        drop(pack);
        let bytes = counter.load(Ordering::Relaxed);
        let (from, at_warmup) = after_warmup.unwrap_or((start, 0));
        self.update(|live| {
            if upload { live.up_mbps = 0.0 } else { live.down_mbps = 0.0 }
        });
        Ok((bytes - at_warmup, Instant::now().duration_since(from), bytes))
    }

    fn download(&self, streams: usize, seconds: u64, loaded_ping: bool) -> Result<(), String> {
        let samples: Arc<Mutex<Vec<f64>>> = Arc::default();
        let mut pinger = Pack::default();
        let mut reader = None;
        if loaded_ping {
            let mut child = self.ping_process(200)?;
            reader = Some(Self::ping_samples(&mut child, samples.clone(), self.live.clone()));
            pinger.0.push(child);
        }
        let (bytes, elapsed, total) = self.sustain(seconds, streams, false, |_, _| {})?;
        drop(pinger);
        if let Some(reader) = reader {
            let _ = reader.join();
        }
        if total < 1_000_000 {
            return Err(if self.refused.load(Ordering::Relaxed) { "источники загрузки отказали".into() } else { "ничего не загрузилось".into() });
        }
        let rate = mbps(bytes, elapsed);
        let mut loaded = samples.lock().map(|s| s.clone()).unwrap_or_default();
        let source = self.refused.load(Ordering::Relaxed).then(|| "Hetzner, Нюрнберг: Cloudflare временно ограничил загрузки".to_owned());
        self.update(|live| {
            if source.is_some() {
                live.results.download_source = source;
            }
            if streams == 1 {
                live.results.download_single_mbps = Some(rate);
            } else {
                live.results.download_mbps = Some(rate);
            }
            if live.results.download_mbps.is_none() {
                live.results.download_mbps = Some(rate);
            }
            if loaded_ping {
                live.results.loaded_ping_ms = median(&mut loaded);
            }
            live.results.bytes_down += total;
        });
        Ok(())
    }

    fn upload(&self, streams: usize, seconds: u64) -> Result<(), String> {
        let (bytes, elapsed, total) = self.sustain(seconds, streams, true, |_, _| {})?;
        if total == 0 {
            return Err("ничего не отправилось".into());
        }
        let rate = mbps(bytes, elapsed);
        self.update(|live| {
            live.results.upload_mbps = Some(rate);
            live.results.bytes_up += total;
        });
        Ok(())
    }

    /// Short requests, each on a new connection, several at a time: what
    /// a browser or a chat app opening many small resources does.
    fn burst(&self, count: usize, parallel: usize, bytes: u64) -> Result<Child, String> {
        let mut command = self.curl();
        command.args(["--http1.1", "-H", "Connection: close", "--parallel", "--parallel-max", &parallel.to_string(), "-w", "%{stderr}%{time_total} %{size_download}\\n"]);
        for _ in 0..count {
            command.args(["-o", NULL, &format!("{DOWN}{bytes}")]);
        }
        command.stdout(Stdio::null()).stderr(Stdio::piped()).spawn().map_err(|e| format!("curl: {e}"))
    }

    fn requests(&self, count: usize, parallel: usize, bytes: u64) -> Result<(), String> {
        let start = Instant::now();
        let mut pack = Pack::default();
        let mut child = self.burst(count, parallel, bytes)?;
        let stdout = child.stderr.take().ok_or("curl без вывода")?;
        pack.0.push(child);
        let mut times = Vec::new();
        let mut received = 0_u64;
        for line in std::io::BufRead::lines(std::io::BufReader::new(stdout)).map_while(Result::ok) {
            if self.cancelled() {
                break;
            }
            let mut parts = line.split_whitespace();
            let (Some(Ok(seconds)), Some(Ok(size))) = (parts.next().map(str::parse::<f64>), parts.next().map(str::parse::<u64>)) else { continue };
            if size == 0 {
                continue;
            }
            times.push(seconds * 1000.0);
            received += size;
            let done = times.len();
            let rate = mbps(received, start.elapsed());
            self.update(|live| {
                live.progress = done as f64 / count as f64;
                live.ping_ms = Some(seconds * 1000.0);
                live.down_mbps = rate;
            });
        }
        drop(pack);
        let elapsed = start.elapsed().as_secs_f64();
        if times.is_empty() {
            return Err("запросы не прошли".into());
        }
        let done = times.len();
        let middle = median(&mut times);
        self.update(|live| {
            live.results.requests_per_second = Some(done as f64 / elapsed);
            live.results.request_ms = middle;
            live.results.bytes_down += received;
            live.down_mbps = 0.0;
        });
        Ok(())
    }

    /// Two long downloads, a stream of short requests and pings at once.
    fn mixed(&self, seconds: u64) -> Result<(), String> {
        let samples: Arc<Mutex<Vec<f64>>> = Arc::default();
        let mut helpers = Pack::default();
        let mut child = self.ping_process(200)?;
        let ping_reader = Self::ping_samples(&mut child, samples.clone(), self.live.clone());
        helpers.0.push(child);
        let times: Arc<Mutex<Vec<f64>>> = Arc::default();
        let mut burst = self.burst(400, 4, 50_000)?;
        if let Some(stdout) = burst.stderr.take() {
            let times = times.clone();
            std::thread::spawn(move || {
                for line in std::io::BufRead::lines(std::io::BufReader::new(stdout)).map_while(Result::ok) {
                    if let Some(Ok(seconds)) = line.split_whitespace().next().map(str::parse::<f64>) {
                        if let Ok(mut times) = times.lock() {
                            times.push(seconds * 1000.0);
                        }
                    }
                }
            });
        }
        helpers.0.push(burst);
        let (bytes, elapsed, total) = self.sustain(seconds, 2, false, |_, _| {})?;
        drop(helpers);
        if total < 1_000_000 {
            return Err("ничего не загрузилось".into());
        }
        let _ = ping_reader.join();
        let mut loaded = samples.lock().map(|s| s.clone()).unwrap_or_default();
        let mut requests = times.lock().map(|t| t.clone()).unwrap_or_default();
        self.update(|live| {
            live.results.mixed_download_mbps = Some(mbps(bytes, elapsed));
            live.results.mixed_request_ms = median(&mut requests);
            live.results.loaded_ping_ms = median(&mut loaded);
            live.results.bytes_down += total;
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_has_steps_and_unknown_kinds_are_refused() {
        for kind in ["quick", "full", "bandwidth", "browsing", "mixed", "latency"] {
            assert!(!plan(kind).unwrap().is_empty(), "{kind}");
        }
        assert!(plan("other").is_none());
        assert_eq!(Step::Download { streams: 6, seconds: 1, loaded_ping: true }.title(), "Загрузка, 6 потоков");
    }

    #[test]
    fn statistics_of_samples() {
        assert_eq!(median(&mut [3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&mut [4.0, 1.0, 2.0, 3.0]), Some(2.5));
        assert_eq!(median(&mut []), None);
        assert_eq!(jitter(&[10.0, 14.0, 12.0]), Some(3.0));
        assert_eq!(jitter(&[10.0]), None);
        assert!((mbps(1_000_000, Duration::from_secs(1)) - 8.0).abs() < 1e-9);
    }

    /// Needs a tunnel with its SOCKS proxy on 127.0.0.1:1080.
    #[test]
    #[ignore]
    fn live_run_through_the_local_tunnel() {
        let kind = std::env::var("SPEEDTEST_KIND").unwrap_or_else(|_| "quick".into());
        let mut tests = SpeedTests::default();
        let (sender, receiver) = std::sync::mpsc::channel();
        let target = Target { host: "127.0.0.1".into(), port: 1080, server: "test".into(), route: vec![], country: String::new() };
        tests.start(&kind, target, move |run| sender.send(run).unwrap()).unwrap();
        while tests.status().running {
            let live = tests.status();
            eprintln!("step {}/{} {:>3.0}% down {:>7.1} up {:>7.1} ping {:?}", live.step + 1, live.steps.len(), live.progress * 100.0, live.down_mbps, live.up_mbps, live.ping_ms.map(|p| p.round()));
            std::thread::sleep(Duration::from_secs(1));
        }
        let run = receiver.recv().unwrap();
        eprintln!("{run:#?}");
        assert!(run.error.is_empty(), "{}", run.error);
    }

    /// Needs a tunnel on 127.0.0.1:1080: downloads switch to the fallback.
    #[test]
    #[ignore]
    fn live_download_from_the_fallback_when_cloudflare_refuses() {
        let live: Arc<Mutex<Live>> = Arc::default();
        let tester = Tester { proxy: "127.0.0.1:1080".into(), host: "127.0.0.1".into(), port: 1080, live: live.clone(), cancel: Arc::default(), started: Instant::now(), refused: Arc::new(AtomicBool::new(true)) };
        tester.download(2, 5, false).unwrap();
        let results = live.lock().unwrap().results.clone();
        eprintln!("{results:?}");
        assert!(results.download_mbps.unwrap() > 1.0);
        assert!(results.download_source.unwrap().starts_with("Hetzner"));
    }

    #[test]
    fn history_keeps_measured_runs_and_survives_a_restart() {
        let dir = std::env::temp_dir().join(format!("secret-tunnel-speed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("speedtests.json");
        let mut tests = SpeedTests::load(path.clone());
        tests.finish(Run { id: "empty".into(), ..Default::default() });
        assert!(tests.history().is_empty(), "a run that measured nothing is not kept");
        let results = Results { ping_ms: Some(80.0), download_mbps: Some(95.5), ..Default::default() };
        tests.finish(Run { id: "a".into(), kind: "quick".into(), results, ..Default::default() });
        let reloaded = SpeedTests::load(path);
        assert_eq!(reloaded.history().len(), 1);
        assert_eq!(reloaded.history()[0].results.download_mbps, Some(95.5));
        std::fs::remove_dir_all(dir).unwrap();
    }
}

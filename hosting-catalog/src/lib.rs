//! Public hosting offers and small, user-initiated TCP latency checks.
//! Independent of SSH state: never changes routes, keys, profiles or VPN policy.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{HashSet, VecDeque},
    io::{Read, Write},
    net::IpAddr,
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::{Duration, Instant},
};

pub const REMOTE: &str = "https://openm1ke.github.io/Secret-Tunnel/catalog/hosting-v1.json";
const BUNDLED: &str = include_str!("../../site/catalog/hosting-v1.json");
const MAX_BYTES: usize = 262_144;
static GENERATION: AtomicU64 = AtomicU64::new(0);
static PROBE: Mutex<()> = Mutex::new(());
static DNS_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(8);
static JOBS: Mutex<Jobs> = Mutex::new(Jobs {
    active: None,
    cancelled: VecDeque::new(),
});
struct Jobs {
    active: Option<String>,
    cancelled: VecDeque<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Catalog {
    pub schema_version: u32,
    pub generated_at: String,
    pub offers: Vec<Offer>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Offer {
    pub id: String,
    pub provider_id: String,
    pub provider: String,
    pub company_country: String,
    pub country: String,
    pub city: String,
    pub plan: String,
    pub price_minor: u64,
    pub currency: String,
    pub cpu: u32,
    pub ram_mb: u32,
    pub disk_gb: u32,
    pub port_mbps: Option<u32>,
    pub conditions: String,
    #[serde(default)]
    pub price_note: Option<String>,
    pub verified_at: String,
    pub source_url: String,
    pub order_url: String,
    pub test_host: Option<String>,
    pub test_port: Option<u16>,
    pub test_source_url: String,
    pub test_note: String,
    #[serde(default)]
    pub update_error: Option<String>,
}

fn https(value: &str) -> bool {
    url::Url::parse(value).is_ok_and(|u| {
        u.scheme() == "https"
            && u.host_str().is_some()
            && u.username().is_empty()
            && u.password().is_none()
            && value.len() <= 2048
    })
}
pub fn parse(raw: &str) -> Result<Catalog, String> {
    if raw.len() > MAX_BYTES {
        return Err("Каталог больше 256 KiB".into());
    }
    let c: Catalog = serde_json::from_str(raw).map_err(|_| "Повреждён формат каталога")?;
    if c.schema_version != 1 || c.offers.is_empty() || c.offers.len() > 32 || !date(&c.generated_at)
    {
        return Err("Неподдерживаемая версия или размер каталога".into());
    }
    let mut ids = HashSet::new();
    for o in &c.offers {
        if !ids.insert(&o.id)
            || o.id.is_empty()
            || o.id.len() > 64
            || !o.id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            || o.country.len() != 2
            || o.price_minor == 0
            || o.price_minor > 100_000_000
            || o.price_note.as_ref().is_some_and(|s| s.len() > 240)
            || !["RUB", "EUR", "USD"].contains(&o.currency.as_str())
            || o.cpu == 0
            || o.ram_mb == 0
            || o.disk_gb == 0
            || !match (&o.test_host, o.test_port) {
                (None, None) => true,
                (Some(host), Some(port)) => [80, 443].contains(&port) && valid_host(host),
                _ => false,
            }
            || !date(&o.verified_at)
            || ![&o.source_url, &o.order_url, &o.test_source_url]
                .iter()
                .all(|s| https(s))
            || [
                &o.provider,
                &o.plan,
                &o.city,
                &o.conditions,
                &o.test_note,
                &o.company_country,
            ]
            .iter()
            .any(|s| s.is_empty() || s.len() > 2000)
            || o.port_mbps.is_some_and(|x| x == 0 || x > 100_000)
        {
            return Err("Некорректная карточка каталога".into());
        }
    }
    Ok(c)
}
fn timestamp(s: &str) -> Option<u64> {
    if !s.is_ascii()
        || !(s.len() == 20 && s.ends_with('Z') || s.len() == 25 && s.ends_with("+00:00"))
    {
        return None;
    }
    let b = s.as_bytes();
    if [b[4], b[7], b[10], b[13], b[16]] != *b"--T::" {
        return None;
    }
    let year: u32 = s[0..4].parse().ok()?;
    let month: usize = s[5..7].parse().ok()?;
    let day: u32 = s[8..10].parse().ok()?;
    let hour: u64 = s[11..13].parse().ok()?;
    let min: u64 = s[14..16].parse().ok()?;
    let sec: u64 = s[17..19].parse().ok()?;
    let leap = |y: u32| y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let months = [
        31,
        if leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if !(2020..=2100).contains(&year)
        || !(1..=12).contains(&month)
        || day == 0
        || day > months[month - 1]
        || hour > 23
        || min > 59
        || sec > 59
    {
        return None;
    }
    let days = (1970..year)
        .map(|y| if leap(y) { 366u64 } else { 365 })
        .sum::<u64>()
        + months[..month - 1].iter().map(|d| *d as u64).sum::<u64>()
        + day as u64
        - 1;
    Some(days * 86400 + hour * 3600 + min * 60 + sec)
}
fn date(s: &str) -> bool {
    timestamp(s).is_some()
}
fn catalog_response(catalog: Catalog, source: &str, warning: Option<String>) -> Value {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let stale = catalog
        .offers
        .iter()
        .any(|o| timestamp(&o.verified_at).is_some_and(|t| now.saturating_sub(t) > 7 * 86400));
    let warning = warning.or_else(|| stale.then(|| "Часть тарифов проверена больше недели назад. Обновите каталог и уточните цену у хостера.".into()));
    let mut result = json!({"catalog":catalog,"source":source});
    if let Some(warning) = warning {
        result["warning"] = json!(warning);
    }
    result
}
fn valid_host(s: &str) -> bool {
    if let Ok(ip) = s.parse::<IpAddr>() {
        return public_ip(ip);
    }
    s.len() <= 253
        && s.contains('.')
        && s.split('.').all(|p| {
            !p.is_empty()
                && p.len() <= 63
                && !p.starts_with('-')
                && !p.ends_with('-')
                && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}
/// Reject local, reserved, fake-IP, NAT64 and mapped addresses before connecting.
pub fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            let [a, b, c, _] = v.octets();
            !(a == 0
                || a == 10
                || a == 127
                || a >= 224
                || (a == 100 && (64..128).contains(&b))
                || (a == 169 && b == 254)
                || (a == 172 && (16..32).contains(&b))
                || (a == 192 && (b == 168 || b == 0 || (b == 88 && c == 99)))
                || (a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)))
                || (a == 203 && b == 0 && c == 113))
        }
        IpAddr::V6(v) => {
            let s = v.segments();
            (s[0] & 0xe000) == 0x2000
                && !(s[0] == 0x2001 && s[1] < 0x200)
                && !(s[0] == 0x2001 && s[1] == 0xdb8)
                && s[0] != 0x2002
                && !(s[0] == 0x3fff && s[1] < 0x1000)
        }
    }
}

fn cached(dir: &Path) -> (Catalog, &'static str) {
    let path = dir.join("hosting-catalog-v1.json");
    if let Ok(file) = std::fs::File::open(path) {
        let mut raw = String::new();
        if file
            .take((MAX_BYTES + 1) as u64)
            .read_to_string(&mut raw)
            .is_ok()
        {
            if let Ok(c) = parse(&raw) {
                let bundled = parse(BUNDLED).expect("valid bundled catalog");
                if timestamp(&c.generated_at) >= timestamp(&bundled.generated_at) {
                    return (c, "cache");
                }
            }
        }
    }
    (parse(BUNDLED).expect("valid bundled catalog"), "bundled")
}
fn refresh(dir: &Path) -> Result<Catalog, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .connect_timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("SecretTunnel-Catalog/1.0")
        .build()
        .map_err(|e| e.to_string())?;
    fetch_and_cache(dir, &client, REMOTE)
}
fn fetch_and_cache(
    dir: &Path,
    client: &reqwest::blocking::Client,
    url: &str,
) -> Result<Catalog, String> {
    let response = client
        .get(url)
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|_| "Сетевой каталог недоступен; сохранены прежние данные.")?;
    if !response.status().is_success() {
        return Err("Перенаправление каталога запрещено".into());
    }
    let mut raw = String::new();
    response
        .take((MAX_BYTES + 1) as u64)
        .read_to_string(&mut raw)
        .map_err(|_| "Не удалось прочитать каталог")?;
    let c = parse(&raw)?;
    if timestamp(&c.generated_at) < timestamp(&cached(dir).0.generated_at) {
        return Err("Получен устаревший каталог; сохранены прежние данные.".into());
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    save_cache(dir, raw.as_bytes())?;
    Ok(c)
}
fn save_cache(dir: &Path, bytes: &[u8]) -> Result<(), String> {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp = dir.join(format!("hosting-catalog-{nonce}.tmp"));
    let result = (|| -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, dir.join("hosting-catalog-v1.json"))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result.map_err(|e| format!("Не удалось сохранить каталог: {e}"))
}

#[derive(Debug, Serialize)]
pub struct Measurement {
    id: String,
    method: &'static str,
    median_ms: Option<f64>,
    min_ms: Option<f64>,
    max_ms: Option<f64>,
    successes: usize,
    attempts: usize,
    family: String,
    error: Option<String>,
    address: Option<String>,
}
fn summary(
    id: String,
    mut samples: Vec<f64>,
    attempts: usize,
    family: String,
    error: Option<String>,
) -> Measurement {
    samples.sort_by(f64::total_cmp);
    let n = samples.len();
    let median = if n == 0 {
        None
    } else {
        Some((samples[(n - 1) / 2] + samples[n / 2]) / 2.0)
    };
    Measurement {
        id,
        method: "TCP",
        median_ms: median,
        min_ms: samples.first().copied(),
        max_ms: samples.last().copied(),
        successes: n,
        attempts,
        family,
        error,
        address: None,
    }
}
async fn measure(o: Offer, generation: u64) -> Measurement {
    let (Some(host), Some(port)) = (o.test_host, o.test_port) else {
        return summary(
            o.id,
            vec![],
            0,
            String::new(),
            Some(
                "У хостера нет подтверждённого TCP-узла для замера. Тариф доступен для просмотра."
                    .into(),
            ),
        );
    };
    let resolved = tokio::time::timeout(Duration::from_secs(3), async move {
        let permit = DNS_SLOTS.acquire().await.map_err(|_| ())?;
        tokio::task::spawn_blocking(move || {
            // Cancellation cannot stop getaddrinfo. Hold the slot inside the
            // resolver task so repeated cancelled jobs cannot grow threads.
            let _permit = permit;
            std::net::ToSocketAddrs::to_socket_addrs(&(host.as_str(), port))
                .map(|items| items.take(32).collect::<Vec<_>>())
                .map_err(|_| ())
        })
        .await
        .map_err(|_| ())?
    })
    .await;
    let addresses: Vec<_> = match resolved {
        Ok(Ok(v)) => v,
        _ => {
            return summary(
                o.id,
                vec![],
                0,
                String::new(),
                Some("DNS не ответил за 3 с".into()),
            )
        }
    };
    // A single local answer invalidates the probe, rather than measuring a DNS interception endpoint.
    if addresses.is_empty() || addresses.iter().any(|a| !public_ip(a.ip())) {
        return summary(
            o.id,
            vec![],
            0,
            String::new(),
            Some(
                "DNS вернул локальный или Fake-IP адрес. Отключите другой VPN/прокси и повторите."
                    .into(),
            ),
        );
    }
    let mut chosen = None;
    let mut samples = vec![];
    let mut attempts = 0;
    for _ in 0..5 {
        if GENERATION.load(Ordering::SeqCst) != generation {
            break;
        }
        attempts += 1;
        let start = Instant::now();
        let target = chosen
            .map(|a| vec![a])
            .unwrap_or_else(|| addresses.iter().take(4).copied().collect());
        // First success pins a family/address: subsequent samples are comparable.
        let connect = async {
            for address in target {
                if let Ok(stream) = tokio::time::timeout(
                    Duration::from_millis(1500),
                    tokio::net::TcpStream::connect(address),
                )
                .await
                {
                    if stream.is_ok() {
                        return Some(address);
                    }
                }
            }
            None
        };
        if let Ok(Some(address)) = tokio::time::timeout(Duration::from_millis(2000), connect).await
        {
            chosen = Some(address);
            samples.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let family = chosen
        .map(|a| if a.is_ipv4() { "IPv4" } else { "IPv6" }.to_string())
        .unwrap_or_default();
    let error = if samples.is_empty() {
        Some("Нет ответа TCP на тестовом порту. Это не означает, что VPS недоступен.".into())
    } else {
        None
    };
    let mut result = summary(o.id, samples, attempts, family, error);
    result.address = chosen.map(|a| a.to_string());
    result
}

pub fn cancel() {
    GENERATION.fetch_add(1, Ordering::SeqCst);
}
fn route_signature() -> Vec<IpAddr> {
    // UDP connect only selects a local route; no packet is sent. Keep the
    // source addresses in memory, never in public measurements or the cache.
    [
        ("0.0.0.0:0", "1.1.1.1:443"),
        ("[::]:0", "[2606:4700::1111]:443"),
    ]
    .iter()
    .filter_map(|(bind, target)| {
        let socket = std::net::UdpSocket::bind(bind).ok()?;
        socket.connect(target).ok()?;
        Some(socket.local_addr().ok()?.ip())
    })
    .collect()
}
/// Call from a worker thread on every platform; never from a UI/async runtime thread.
pub fn request(raw: &str, dir: &Path, vpn_active: bool) -> String {
    let result = (|| -> Result<Value, String> {
        if raw.len() > 4096 {
            return Err("Слишком большой запрос".into());
        }
        let req: Value = serde_json::from_str(raw).map_err(|_| "Некорректный запрос")?;
        match req["action"].as_str().unwrap_or("load") {
            "cancel" => {
                if let Some(job) = req["job"]
                    .as_str()
                    .filter(|j| !j.is_empty() && j.len() <= 64)
                {
                    let mut jobs = JOBS.lock().map_err(|e| e.to_string())?;
                    if jobs.active.as_deref() == Some(job) {
                        cancel();
                    }
                    if !jobs.cancelled.iter().any(|x| x == job) {
                        jobs.cancelled.push_back(job.to_string());
                    }
                    while jobs.cancelled.len() > 64 {
                        jobs.cancelled.pop_front();
                    }
                }
                Ok(json!({"cancelled":true}))
            }
            "load" => {
                let (catalog, source) = cached(dir);
                Ok(catalog_response(catalog, source, None))
            }
            "refresh" => match refresh(dir) {
                Ok(c) => Ok(catalog_response(c, "network", None)),
                Err(e) => {
                    let (catalog, source) = cached(dir);
                    Ok(catalog_response(catalog, source, Some(e)))
                }
            },
            "probe" => {
                if vpn_active {
                    return Err("Остановите туннель перед проверкой хостеров. Перед этим закройте защищаемые приложения: после остановки защита от прямого выхода может прекратиться.".into());
                }
                let _guard = PROBE
                    .try_lock()
                    .map_err(|_| "Предыдущий замер ещё завершается")?;
                let job = req["job"]
                    .as_str()
                    .filter(|j| !j.is_empty() && j.len() <= 64)
                    .ok_or("Не указан номер замера")?;
                let generation = {
                    let mut jobs = JOBS.lock().map_err(|e| e.to_string())?;
                    if jobs.cancelled.iter().any(|x| x == job) {
                        return Ok(json!({"cancelled":true}));
                    }
                    jobs.active = Some(job.to_string());
                    GENERATION.fetch_add(1, Ordering::SeqCst) + 1
                };
                let catalog = cached(dir).0;
                let ids: Vec<_> = req["ids"]
                    .as_array()
                    .ok_or("Не выбраны хостеры")?
                    .iter()
                    .filter_map(Value::as_str)
                    .collect();
                if ids.is_empty() || ids.len() > 8 {
                    return Err("Выберите от 1 до 8 узлов".into());
                }
                let offers: Vec<_> = catalog
                    .offers
                    .into_iter()
                    .filter(|o| ids.contains(&o.id.as_str()))
                    .collect();
                if offers.len() != ids.len() {
                    return Err("Неизвестный или повторный узел".into());
                }
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| e.to_string())?;
                let route = route_signature();
                let mut network_changed = false;
                let results = runtime.block_on(async {
                    let mut tasks = tokio::task::JoinSet::new();
                    for offer in offers { tasks.spawn(measure(offer, generation)); }
                    let mut results = vec![];
                    let mut checked_route = Instant::now();
                    loop {
                        tokio::select! {
                            r = tasks.join_next() => match r { Some(Ok(m)) => results.push(m), Some(Err(_)) => {}, None => break },
                            _ = tokio::time::sleep(Duration::from_millis(50)) => {
                                if GENERATION.load(Ordering::SeqCst) != generation { tasks.abort_all(); break; }
                                if checked_route.elapsed() >= Duration::from_millis(500) {
                                    checked_route = Instant::now();
                                    if route_signature() != route { network_changed = true; tasks.abort_all(); break; }
                                }
                            }
                        }
                    }
                    results
                });
                // System DNS lookups may finish after our deadline. Do not wait for
                // those blocking resolver threads while returning a cancelled job.
                runtime.shutdown_background();
                if network_changed {
                    Ok(json!({"cancelled":true,"warning":"Сеть изменилась. Повторите замер."}))
                } else if GENERATION.load(Ordering::SeqCst) != generation {
                    Ok(json!({"cancelled":true}))
                } else {
                    Ok(json!({"results":results}))
                }
            }
            _ => Err("Неизвестное действие".into()),
        }
    })();
    result
        .unwrap_or_else(|error| json!({"error":error}))
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn price_note_is_optional_but_bounded() {
        let mut old: Value = serde_json::from_str(BUNDLED).unwrap();
        old["offers"][0]
            .as_object_mut()
            .unwrap()
            .remove("price_note");
        let parsed = parse(&old.to_string()).unwrap();
        assert!(parsed.offers[0].price_note.is_none());
        assert_eq!(parsed.offers[0].price_minor, 649);
        old["offers"][0]["price_note"] = json!("x".repeat(241));
        assert!(parse(&old.to_string()).is_err());
    }
    #[test]
    fn bundled_and_tampered_catalogs() {
        let parsed = parse(BUNDLED).unwrap();
        for provider in [
            "ovh", "itldc", "timeweb", "firstvds", "iphoster", "vultr", "hetzner",
        ] {
            assert!(parsed.offers.iter().any(|o| o.provider_id == provider));
        }
        let base: Value = serde_json::from_str(BUNDLED).unwrap();
        for (field, value) in [
            ("price_minor", json!(0)),
            ("test_host", json!("127.0.0.1")),
            ("test_port", json!(22)),
            ("order_url", json!("javascript:alert(1)")),
        ] {
            let mut bad = base.clone();
            bad["offers"][0][field] = value;
            assert!(parse(&bad.to_string()).is_err());
        }
        let mut duplicate = base.clone();
        duplicate["offers"][1]["id"] = duplicate["offers"][0]["id"].clone();
        assert!(parse(&duplicate.to_string()).is_err());
    }
    #[test]
    fn unmeasurable_offer_never_opens_a_network_connection() {
        let catalog = parse(BUNDLED).unwrap();
        let offer = catalog
            .offers
            .into_iter()
            .find(|o| o.provider_id == "iphoster")
            .unwrap();
        assert!(offer.test_host.is_none() && offer.test_port.is_none());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = runtime.block_on(measure(offer, 0));
        assert_eq!(result.attempts, 0);
        assert!(result.median_ms.is_none());
        assert!(result.error.unwrap().contains("нет подтверждённого"));
        let mut incomplete: Value = serde_json::from_str(BUNDLED).unwrap();
        incomplete["offers"][0]["test_host"] = Value::Null;
        assert!(parse(&incomplete.to_string()).is_err());
    }
    #[test]
    fn rejects_private_fake_and_translated_ips() {
        for ip in [
            "127.0.0.1",
            "10.0.0.1",
            "198.18.2.1",
            "198.19.255.1",
            "192.168.1.1",
            "100.64.1.1",
            "::1",
            "::ffff:8.8.8.8",
            "64:ff9b::808:808",
            "fc00::1",
            "2001:db8::1",
            "2002:7f00:1::",
        ] {
            assert!(!public_ip(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["8.8.8.8", "195.123.217.176", "2606:4700::1111"] {
            assert!(public_ip(ip.parse().unwrap()), "{ip}");
        }
    }
    #[test]
    fn even_median_and_partial_response() {
        let m = summary("x".into(), vec![10., 30., 20., 50.], 5, "IPv4".into(), None);
        assert_eq!(m.median_ms, Some(25.));
        assert_eq!(m.successes, 4);
        assert_eq!(m.attempts, 5);
    }
    #[test]
    fn active_vpn_prevents_probe_and_load_preserves_settings() {
        let path = std::env::temp_dir().join("st-catalog-no-write");
        let r: Value =
            serde_json::from_str(&request(r#"{"action":"probe","ids":["ovh"]}"#, &path, true))
                .unwrap();
        assert!(r["error"].as_str().unwrap().contains("Остановите"));
        assert!(!path.exists());
    }
    #[test]
    fn cancellation_before_worker_start_prevents_network_work() {
        let dir = std::env::temp_dir();
        request(
            r#"{"action":"cancel","job":"cancel-before-start-test"}"#,
            &dir,
            false,
        );
        let r: Value = serde_json::from_str(&request(
            r#"{"action":"probe","ids":["ovh"],"job":"cancel-before-start-test"}"#,
            &dir,
            false,
        ))
        .unwrap();
        assert_eq!(r["cancelled"], true);
        assert!(r["results"].is_null());
    }
    #[test]
    fn atomically_replaces_cache_and_ignores_malformed_data() {
        let dir = std::env::temp_dir().join(format!("st-catalog-cache-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        save_cache(&dir, BUNDLED.as_bytes()).unwrap();
        assert_eq!(cached(&dir).1, "cache");
        save_cache(&dir, b"broken").unwrap();
        assert_eq!(cached(&dir).1, "bundled");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn http_fixture_refresh_and_bad_response_preserve_last_good_copy() {
        fn serve(body: Vec<u8>) -> (String, std::thread::JoinHandle<()>) {
            let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", server.local_addr().unwrap());
            let thread = std::thread::spawn(move || {
                let (mut stream, _) = server.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = [0; 8192];
                let _ = stream.read(&mut request);
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(&body);
            });
            (url, thread)
        }
        let dir = std::env::temp_dir().join(format!("st-catalog-http-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(2))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let mut data: Value = serde_json::from_str(BUNDLED).unwrap();
        data["offers"][0]["price_minor"] = json!(777);
        let (url, thread) = serve(data.to_string().into_bytes());
        assert_eq!(
            fetch_and_cache(&dir, &client, &url).unwrap().offers[0].price_minor,
            777
        );
        thread.join().unwrap();
        for body in [b"broken".to_vec(), vec![b'x'; MAX_BYTES + 1]] {
            let (url, thread) = serve(body);
            assert!(fetch_and_cache(&dir, &client, &url).is_err());
            thread.join().unwrap();
            assert_eq!(cached(&dir).0.offers[0].price_minor, 777);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}

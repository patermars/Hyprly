use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::Write,
    os::{
        fd::AsRawFd,
        unix::fs::{OpenOptionsExt, PermissionsExt},
    },
    path::Path,
    sync::mpsc::{self, RecvTimeoutError, Sender},
    time::Duration,
};
use time::OffsetDateTime;

const BASE: &str = "https://counterapi.com";
const NAMESPACE: &str = "patermars-hyprly";
const ACTION: &str = "monthly-connected";
const RETRY_SECONDS: i64 = 3600;

pub struct Reporter(Sender<(bool, OffsetDateTime)>);

impl Reporter {
    pub fn start() -> Self {
        let (tx, rx) = mpsc::channel();
        // Failure to create a reporter must never affect the desktop app.
        let _ = std::thread::Builder::new()
            .name("monthly-activity".into())
            .spawn(move || {
                let Some(state_dir) = crate::config::xdg_dir("XDG_STATE_HOME", ".local/state")
                else {
                    return;
                };
                let root = state_dir.join("hyprly");
                let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                else {
                    return;
                };
                let Ok(client) = reqwest::Client::builder()
                    .https_only(true)
                    .redirect(reqwest::redirect::Policy::none())
                    .timeout(Duration::from_secs(8))
                    .user_agent("Hyprly-Activity/1")
                    .build()
                else {
                    return;
                };
                let mut session = Session::default();
                let mut last_error: Option<std::time::Instant> = None;
                loop {
                    match rx.recv_timeout(Duration::from_secs(30)) {
                        Ok((value, at)) => session.connection(value, at),
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                    for (value, at) in rx.try_iter() {
                        session.connection(value, at);
                    }
                    let now = OffsetDateTime::now_utc();
                    session.poll(now);
                    if let Err(error) =
                        tick(&root, &mut session.observed, now.unix_timestamp(), |m| {
                            runtime.block_on(send(&client, BASE, m))
                        })
                    {
                        // Avoid repeatedly logging local storage errors on every timer tick.
                        if last_error.is_none_or(|at| at.elapsed() >= Duration::from_secs(3600)) {
                            eprintln!("Monthly activity report deferred: {error}");
                            last_error = Some(std::time::Instant::now());
                        }
                    }
                }
            });
        Self(tx)
    }

    pub fn connection(&self, connected: bool) {
        let _ = self.0.send((connected, OffsetDateTime::now_utc()));
    }
}

#[derive(Default)]
struct Session {
    connected: bool,
    observed: BTreeSet<String>,
}

impl Session {
    fn connection(&mut self, connected: bool, at: OffsetDateTime) {
        self.connected = connected;
        if connected {
            self.observed.insert(month(at));
        }
    }

    fn poll(&mut self, now: OffsetDateTime) {
        if self.connected {
            self.observed.insert(month(now));
        }
    }
}

fn month(at: OffsetDateTime) -> String {
    format!("{:04}-{:02}", at.year(), u8::from(at.month()))
}

fn valid_month(value: &str) -> bool {
    value.len() == 7
        && value.as_bytes()[4] == b'-'
        && value[..4].bytes().all(|b| b.is_ascii_digit())
        && value[5..].bytes().all(|b| b.is_ascii_digit())
        && value[5..]
            .parse::<u8>()
            .is_ok_and(|m| (1..=12).contains(&m))
}

#[derive(Serialize, Deserialize)]
struct Ledger {
    version: u8,
    completed: BTreeSet<String>,
    // Next permitted attempt, persisted before each request (including failed ones).
    pending: BTreeMap<String, i64>,
}

impl Default for Ledger {
    fn default() -> Self {
        Self {
            version: 1,
            completed: BTreeSet::new(),
            pending: BTreeMap::new(),
        }
    }
}

impl Ledger {
    fn load(root: &Path) -> Result<Self> {
        let bytes = match fs::read(root.join("activity.json")) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e.into()),
        };
        let ledger: Self = serde_json::from_slice(&bytes).context("invalid activity state")?;
        if ledger.version != 1
            || !ledger
                .completed
                .iter()
                .chain(ledger.pending.keys())
                .all(|m| valid_month(m))
            || ledger.pending.keys().any(|m| ledger.completed.contains(m))
        {
            bail!("unsupported or invalid activity state");
        }
        Ok(ledger)
    }

    fn save(&self, root: &Path) -> Result<()> {
        let temporary = root.join("activity.json.tmp");
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        file.write_all(&serde_json::to_vec(self)?)?;
        file.sync_all()?;
        fs::rename(temporary, root.join("activity.json"))?;
        File::open(root)?.sync_all()?;
        Ok(())
    }
}

struct Lock(File);
impl Drop for Lock {
    fn drop(&mut self) {
        // SAFETY: the descriptor belongs to this live File.
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

fn lock(root: &Path) -> Result<Option<Lock>> {
    fs::create_dir_all(root)?;
    fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(root.join("activity.lock"))?;
    // SAFETY: flock operates on the descriptor of the live file above.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::WouldBlock {
            return Ok(None);
        }
        return Err(error.into());
    }
    Ok(Some(Lock(file)))
}

fn tick(
    root: &Path,
    observed: &mut BTreeSet<String>,
    now: i64,
    mut report: impl FnMut(&str) -> Result<()>,
) -> Result<()> {
    let Some(_lock) = lock(root)? else {
        return Ok(());
    };
    let mut ledger = Ledger::load(root)?;
    let mut changed = false;
    for m in observed.iter().filter(|m| valid_month(m)) {
        if !ledger.completed.contains(m) && !ledger.pending.contains_key(m) {
            ledger.pending.insert(m.clone(), 0);
            changed = true;
        }
    }
    if changed {
        ledger.save(root)?;
    }
    observed.clear();
    // One request per tick limits bursts when delivering older pending months.
    let due = ledger
        .pending
        .iter()
        .find(|(_, retry)| **retry <= now)
        .map(|(m, _)| m.clone());
    if let Some(m) = due {
        ledger
            .pending
            .insert(m.clone(), now.saturating_add(RETRY_SECONDS));
        ledger.save(root)?;
        report(&m)?;
        ledger.pending.remove(&m);
        ledger.completed.insert(m);
        ledger.save(root)?;
    }
    Ok(())
}

#[derive(Deserialize)]
struct Counter {
    value: u64,
    ns: String,
    action: String,
    key: String,
}

async fn send(client: &reqwest::Client, base: &str, month: &str) -> Result<()> {
    if !valid_month(month) {
        bail!("invalid report month");
    }
    // Counting is the API default. Reserve readOnly=true for viewing statistics.
    // The returned value may lag behind the accepted write.
    let url = format!("{base}/api/{NAMESPACE}/{ACTION}/{month}");
    let mut response = client.get(url).send().await?.error_for_status()?;
    if !response.status().is_success() {
        bail!("unexpected counter redirect");
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len() + chunk.len() > 8192 {
            bail!("counter response too large");
        }
        body.extend_from_slice(&chunk);
    }
    let counter: Counter = serde_json::from_slice(&body).context("invalid counter response")?;
    if counter.ns != NAMESPACE || counter.action != ACTION || counter.key != month {
        bail!("counter response identity mismatch");
    }
    let _ = counter.value;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            Self(std::env::temp_dir().join(format!(
                "hyprly-activity-{}-{}-{}",
                std::process::id(),
                OffsetDateTime::now_utc().unix_timestamp_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            )))
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn observed(m: &str) -> BTreeSet<String> {
        BTreeSet::from([m.into()])
    }

    #[test]
    fn once_per_month_across_restarts_and_rollover() {
        let root = Temp::new();
        let mut sent = Vec::new();
        for m in ["2026-11", "2026-11", "2026-12", "2026-11"] {
            tick(&root.0, &mut observed(m), 100, |m| {
                sent.push(m.to_owned());
                Ok(())
            })
            .unwrap();
        }
        assert_eq!(sent, ["2026-11", "2026-12"]);
        assert_eq!(
            fs::metadata(root.0.join("activity.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn no_connection_creates_no_report() {
        let root = Temp::new();
        tick(&root.0, &mut BTreeSet::new(), 100, |_| {
            panic!("unconnected launch reported")
        })
        .unwrap();
    }

    #[test]
    fn failures_are_persisted_throttled_and_delivered_after_restart() {
        let root = Temp::new();
        assert!(tick(&root.0, &mut observed("2026-11"), 100, |_| bail!("offline")).is_err());
        assert!(!Ledger::load(&root.0).unwrap().completed.contains("2026-11"));
        tick(&root.0, &mut observed("2026-11"), 3699, |_| {
            panic!("retried early")
        })
        .unwrap();
        let mut sent = Vec::new();
        tick(&root.0, &mut BTreeSet::new(), 3700, |m| {
            sent.push(m.to_owned());
            Ok(())
        })
        .unwrap();
        assert_eq!(sent, ["2026-11"]);
    }

    #[test]
    fn concurrent_reporter_retains_observations_until_lock_available() {
        let root = Temp::new();
        let guard = lock(&root.0).unwrap().unwrap();
        let mut pending = observed("2026-11");
        tick(&root.0, &mut pending, 100, |_| panic!("concurrent write")).unwrap();
        assert!(!pending.is_empty());
        drop(guard);
        tick(&root.0, &mut pending, 100, |_| Ok(())).unwrap();
        assert!(pending.is_empty());
    }

    #[test]
    fn corrupt_or_unavailable_state_never_sends() {
        let root = Temp::new();
        fs::create_dir_all(&root.0).unwrap();
        fs::write(root.0.join("activity.json"), "broken").unwrap();
        assert!(tick(&root.0, &mut observed("2026-11"), 100, |_| panic!(
            "corrupt state reported"
        ))
        .is_err());
        let blocked = root.0.join("file");
        fs::write(&blocked, "not a directory").unwrap();
        assert!(tick(&blocked, &mut observed("2026-11"), 100, |_| panic!(
            "unavailable state reported"
        ))
        .is_err());
    }

    #[test]
    fn utc_calendar_months_include_year_rollover() {
        let jan = time::Date::from_calendar_date(2027, time::Month::January, 1)
            .unwrap()
            .midnight()
            .assume_utc();
        assert_eq!(month(jan), "2027-01");
        assert_eq!(month(jan - time::Duration::seconds(1)), "2026-12");
        for invalid in ["2026-00", "2026-13", "2026-1", "../../x", "abcd-11"] {
            assert!(!valid_month(invalid));
        }
    }

    #[test]
    fn only_confirmed_sessions_qualify_including_continuous_month_rollover() {
        let jan = time::Date::from_calendar_date(2027, time::Month::January, 1)
            .unwrap()
            .midnight()
            .assume_utc();
        let dec = jan - time::Duration::seconds(1);
        let mut session = Session::default();
        session.poll(dec);
        session.connection(false, dec);
        assert!(session.observed.is_empty());
        session.connection(true, dec);
        session.poll(jan);
        assert_eq!(
            session.observed,
            BTreeSet::from(["2026-12".into(), "2027-01".into()])
        );
        session.observed.clear();
        session.connection(false, jan);
        session.poll(jan + time::Duration::days(40));
        assert!(session.observed.is_empty());
        // A brief successful connection still qualifies even if it disconnects before delivery.
        session.connection(true, jan);
        session.connection(false, jan);
        assert_eq!(session.observed, observed("2027-01"));
    }

    async fn mock_response(status: &str, body: &str, delay: Duration) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            let n = stream.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..n]);
            assert!(
                request.starts_with("GET /api/patermars-hyprly/monthly-connected/2026-11 HTTP/1.1")
            );
            assert!(!request.to_ascii_lowercase().contains("authorization:"));
            tokio::time::sleep(delay).await;
            let _ = stream.write_all(response.as_bytes()).await;
        });
        format!("http://{address}")
    }

    #[tokio::test]
    async fn validates_response_and_handles_errors_without_marking_success() {
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(1))
            .build()
            .unwrap();
        for (status, body, success) in [
            (
                "200 OK",
                r#"{"value":1,"ns":"patermars-hyprly","action":"monthly-connected","key":"2026-11"}"#,
                true,
            ),
            (
                "200 OK",
                r#"{"value":1,"ns":"other","action":"monthly-connected","key":"2026-11"}"#,
                false,
            ),
            ("200 OK", "<html>error</html>", false),
            ("503 Unavailable", "offline", false),
        ] {
            let base = mock_response(status, body, Duration::ZERO).await;
            assert_eq!(send(&client, &base, "2026-11").await.is_ok(), success);
        }
        let base = mock_response("200 OK", "{}", Duration::from_secs(1)).await;
        let impatient = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_millis(20))
            .build()
            .unwrap();
        assert!(send(&impatient, &base, "2026-11").await.is_err());
    }
}

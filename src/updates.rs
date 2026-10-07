//! Self-update through fastframe-update, from the repository's GitHub
//! releases.
//!
//! A newer release is offered in the About dialog. Nothing is downloaded
//! until the user asks, and nothing is installed until they ask again: the
//! app quits to be replaced, so it waits for a moment they choose. A check
//! that fails (no network) is logged and tried again later.
//!
//! A release is installed only when its `checksums.txt` carries a valid
//! signature by the key compiled in here. See packaging/UPDATE_SIGNING.md.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, SystemTime};

use fastframe_update::{
    MacConfig, Prepared, Release, ReqwestTransport, Source, UpdateConfig, Updater,
};

/// How often to look for a newer release.
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

pub const CONFIG: UpdateConfig = UpdateConfig {
    macos: MacConfig {
        bundle_ids: &["dev.tabletist.Tabletist"],
        executable_names: &[],
        legacy_bundle_names: &[],
    },
    publisher_key: Some(include_str!("../assets/update-public-key.hex")),
    ..UpdateConfig::new(
        "igor-alexandrov/tabletist",
        "Tabletist",
        "tabletist",
        env!("CARGO_PKG_VERSION"),
    )
};

/// Names a feed on this computer to read instead of GitHub, for trying an
/// update end to end: `http://127.0.0.1:<port>`. fastframe-update takes no
/// other host, and a release from it must carry the same signature.
pub const FEED_VARIABLE: &str = "TABLETIST_UPDATE_FEED";

fn updater() -> anyhow::Result<Updater> {
    let transport = ReqwestTransport::new(reqwest::blocking::Client::builder())?;
    let updater = Updater::new(CONFIG, transport);
    match std::env::var(FEED_VARIABLE) {
        Ok(feed) => Ok(updater.with_source(Source::local(&feed)?)),
        Err(_) => Ok(updater),
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Phase {
    Idle,
    /// A newer release, not downloaded.
    Available {
        version: String,
    },
    Downloading {
        version: String,
    },
    /// Downloaded and verified: installed when the user restarts.
    Ready {
        version: String,
    },
    /// Being handed to fastframe's helper.
    Starting {
        version: String,
    },
    /// The helper has it: the app quits so it can install.
    Installing,
    Failed {
        version: String,
    },
}

/// What the user can do about a newer release.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Download it.
    Download,
    Downloading,
    /// Quit, to be replaced by it.
    Restart,
    /// Quitting.
    Restarting,
    /// The download or the handoff failed: download it again.
    Retry,
}

/// What the About dialog shows about an update.
#[derive(Clone, Debug, PartialEq)]
pub struct Offer {
    pub version: String,
    pub step: Step,
}

/// How the last look for a newer release ended.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Checked {
    /// Not looked yet (or updates are off, as in `--demo`).
    #[default]
    Never,
    Checking,
    /// This is the newest release.
    UpToDate,
    /// A newer release this copy can install: see the [`Offer`].
    Available,
    /// A newer release this copy will not replace itself with, and why
    /// (run from the disk image, owned by a package manager).
    Elsewhere {
        version: String,
        reason: String,
    },
    /// No answer: no network, or GitHub said no.
    Unreachable(String),
}

/// What one look found.
enum Found {
    Newer(Release),
    Nothing,
    Elsewhere { version: String, reason: String },
    Unreachable(String),
}

/// Called when something changed that a frame should show.
type Wake = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone)]
pub struct Updates {
    phase: Arc<Mutex<Phase>>,
    release: Arc<Mutex<Option<Release>>>,
    prepared: Arc<Mutex<Option<Prepared>>>,
    checked: Arc<Mutex<Checked>>,
    /// `None` where nothing is checked: `--demo` and the tests.
    wake: Option<Wake>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().expect("an update thread panicked")
}

impl Updates {
    /// No checks: `--demo` and the tests, which must not reach the network.
    pub fn disabled() -> Self {
        Self {
            phase: Arc::new(Mutex::new(Phase::Idle)),
            release: Arc::new(Mutex::new(None)),
            prepared: Arc::new(Mutex::new(None)),
            checked: Arc::new(Mutex::new(Checked::Never)),
            wake: None,
        }
    }

    /// Looks at once, and again every six hours. `wake` asks for a frame.
    pub fn spawn(wake: impl Fn() + Send + Sync + 'static) -> Self {
        let updates = Self {
            wake: Some(Arc::new(wake)),
            ..Self::disabled()
        };
        let looker = updates.clone();
        thread::spawn(move || {
            if let Some(folder) = installation_folder() {
                clean_staging(&folder, SystemTime::now());
            }
            loop {
                looker.look();
                thread::sleep(CHECK_EVERY);
            }
        });
        updates
    }

    /// Looks now, because the user asked. Nothing happens where checks are
    /// off, while a look is under way, or once an update is on its way in.
    pub fn check_now(&self) {
        if !self.can_check() {
            return;
        }
        let looker = self.clone();
        thread::spawn(move || looker.look());
    }

    /// Whether the user can ask for a look.
    pub fn can_check(&self) -> bool {
        self.wake.is_some() && *lock(&self.checked) != Checked::Checking && self.may_look()
    }

    /// A look replaces the offer, so none runs once a download started.
    fn may_look(&self) -> bool {
        matches!(
            *lock(&self.phase),
            Phase::Idle | Phase::Available { .. } | Phase::Failed { .. }
        )
    }

    pub fn checked(&self) -> Checked {
        lock(&self.checked).clone()
    }

    pub fn offer(&self) -> Option<Offer> {
        let (version, step) = match lock(&self.phase).clone() {
            Phase::Available { version } => (version, Step::Download),
            Phase::Downloading { version } => (version, Step::Downloading),
            Phase::Ready { version } => (version, Step::Restart),
            Phase::Starting { version } => (version, Step::Restarting),
            Phase::Failed { version } => (version, Step::Retry),
            Phase::Idle | Phase::Installing => return None,
        };
        Some(Offer { version, step })
    }

    /// The helper has the update: the app should quit so it can install.
    pub fn installing(&self) -> bool {
        *lock(&self.phase) == Phase::Installing
    }

    /// Downloads and verifies the offered release, off the UI thread.
    pub fn download(&self) {
        let Some(release) = lock(&self.release).clone() else {
            return;
        };
        {
            let mut phase = lock(&self.phase);
            if !matches!(*phase, Phase::Available { .. } | Phase::Failed { .. }) {
                return;
            }
            *phase = Phase::Downloading {
                version: release.version.clone(),
            };
        }
        // Where checks are off nothing reaches the network.
        if self.wake.is_none() {
            return;
        }
        log::info!("updates: downloading {}", release.version);
        let updates = self.clone();
        thread::spawn(move || {
            let result = updater().and_then(|updater| updater.download(&release, |_, _| {}));
            let version = release.version;
            *lock(&updates.phase) = match result {
                Ok(prepared) => {
                    log::info!("updates: {version} is downloaded and verified");
                    *lock(&updates.prepared) = Some(prepared);
                    Phase::Ready { version }
                }
                Err(error) => {
                    log::warn!("updates: could not download {version}: {error:#}");
                    Phase::Failed { version }
                }
            };
            updates.wake();
        });
    }

    /// Hands the downloaded update to the helper, off the UI thread. Once
    /// it has it, [`Updates::installing`] says so and the app quits.
    pub fn restart(&self) {
        let version = {
            let mut phase = lock(&self.phase);
            let Phase::Ready { version } = phase.clone() else {
                return;
            };
            *phase = Phase::Starting {
                version: version.clone(),
            };
            version
        };
        let Some(prepared) = lock(&self.prepared).take() else {
            *lock(&self.phase) = Phase::Failed { version };
            return;
        };
        let updates = self.clone();
        thread::spawn(move || {
            let result = updater().and_then(|updater| updater.handoff(prepared, Vec::new()));
            *lock(&updates.phase) = match result {
                Ok(()) => {
                    log::info!("updates: {version} is handed to the installer; quitting");
                    Phase::Installing
                }
                Err(error) => {
                    log::warn!("updates: could not install {version}: {error:#}");
                    Phase::Failed { version }
                }
            };
            updates.wake();
        });
    }

    fn wake(&self) {
        if let Some(wake) = &self.wake {
            wake();
        }
    }
}

/// Looking for a release, on a thread of its own.
impl Updates {
    fn look(&self) {
        {
            let mut checked = lock(&self.checked);
            if *checked == Checked::Checking || !self.may_look() {
                return;
            }
            *checked = Checked::Checking;
        }
        self.wake();
        let found = find();
        self.record(found);
        self.wake();
    }

    fn record(&self, found: Found) {
        let checked = match found {
            Found::Newer(found) => {
                // A failed download of the same version stays failed, so the
                // button still says "Try again".
                let mut phase = lock(&self.phase);
                if !matches!(&*phase, Phase::Failed { version } if *version == found.version) {
                    *phase = Phase::Available {
                        version: found.version.clone(),
                    };
                }
                *lock(&self.release) = Some(found);
                Checked::Available
            }
            Found::Nothing => Checked::UpToDate,
            Found::Elsewhere { version, reason } => Checked::Elsewhere { version, reason },
            Found::Unreachable(reason) => Checked::Unreachable(reason),
        };
        *lock(&self.checked) = checked;
    }
}

/// How long a finished update's staging folder is left alone: its helper
/// may still be watching that the new version started.
const STAGING_KEPT_FOR: Duration = Duration::from_secs(10 * 60);

/// Where the installation sits and updates are staged beside it: the folder
/// holding the bundle on macOS, the program's own folder elsewhere.
fn installation_folder() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let bundle = exe
        .ancestors()
        .find(|path| path.extension().is_some_and(|ext| ext == "app"));
    match bundle {
        Some(bundle) => bundle.parent().map(Path::to_path_buf),
        None => exe.parent().map(Path::to_path_buf),
    }
}

/// Whether `name` is a staging folder of this app's updater:
/// `.tabletist-update-` and sixteen lowercase hexadecimal digits.
fn is_staging_name(name: &str) -> bool {
    name.strip_prefix(".tabletist-update-").is_some_and(|id| {
        id.len() == 16
            && id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// Removes what finished updates left beside the installation. Each one
/// keeps the downloaded package and a copy of the previous app, tens of
/// megabytes that nothing reads again once the new version has started.
///
/// Only folders with the updater's exact name, holding its own `handoff.json`
/// and a mark that the update ended (`started` or `result.txt`), and not
/// touched for ten minutes. Returns what was removed.
pub fn clean_staging(folder: &Path, now: SystemTime) -> Vec<PathBuf> {
    let mut removed = Vec::new();
    for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
        let path = entry.path();
        let is_ours = is_staging_name(&entry.file_name().to_string_lossy())
            && entry.file_type().is_ok_and(|kind| kind.is_dir())
            && path.join("handoff.json").is_file()
            && (path.join("started").exists() || path.join("result.txt").exists());
        let settled = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age >= STAGING_KEPT_FOR);
        if is_ours && settled {
            match std::fs::remove_dir_all(&path) {
                Ok(()) => {
                    log::info!(
                        "updates: removed what an earlier update left ({})",
                        path.display()
                    );
                    removed.push(path);
                }
                Err(error) => log::warn!("updates: could not remove {}: {error}", path.display()),
            }
        }
    }
    removed
}

/// Asks GitHub for the latest release, and whether this copy may replace
/// itself with it. Package-managed and development copies may not.
fn find() -> Found {
    let updater = match updater() {
        Ok(updater) => updater,
        Err(error) => {
            log::warn!("updates: {error:#}");
            return Found::Unreachable(format!("{error:#}"));
        }
    };
    let release = match updater.check() {
        Ok(Some(release)) => release,
        Ok(None) => {
            log::info!("updates: this is the newest release");
            return Found::Nothing;
        }
        Err(error) => {
            log::info!("updates: could not check ({error:#})");
            return Found::Unreachable(format!("{error:#}"));
        }
    };
    if let Err(reason) = updater.installation() {
        log::info!(
            "updates: {} is out, but this copy does not update itself ({reason})",
            release.version
        );
        return Found::Elsewhere {
            version: release.version,
            reason: reason.to_string(),
        };
    }
    log::info!("updates: {} is available", release.version);
    Found::Newer(release)
}

#[cfg(test)]
impl Updates {
    /// As after a look that found `version`, for the views' tests.
    pub fn offering(version: &str) -> Self {
        let updates = Self::disabled();
        updates.record(Found::Newer(Release {
            version: version.into(),
            url: String::new(),
        }));
        updates
    }

    /// As after its download: ready to install.
    pub fn downloaded(version: &str) -> Self {
        let updates = Self::disabled();
        *lock(&updates.phase) = Phase::Ready {
            version: version.into(),
        };
        updates
    }

    /// As after a look that ended as `checked`.
    pub fn having_checked(checked: Checked) -> Self {
        let updates = Self::disabled();
        *lock(&updates.checked) = checked;
        updates
    }

    /// As once the helper has the update.
    pub fn handed_over() -> Self {
        let updates = Self::disabled();
        *lock(&updates.phase) = Phase::Installing;
        updates
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_update_config_is_valid() {
        CONFIG.validate().unwrap();
        assert_eq!(CONFIG.current_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(
            format!("https://github.com/{}", CONFIG.repository),
            env!("CARGO_PKG_REPOSITORY")
        );
    }

    #[test]
    fn nothing_is_offered_before_a_check_finds_something() {
        let updates = Updates::disabled();
        assert_eq!(updates.offer(), None);
        assert!(!updates.installing());
    }

    #[test]
    fn each_phase_shows_as_its_step() {
        let updates = Updates::disabled();
        let version = || "0.22.0".to_owned();
        for (phase, step) in [
            (Phase::Available { version: version() }, Step::Download),
            (Phase::Downloading { version: version() }, Step::Downloading),
            (Phase::Ready { version: version() }, Step::Restart),
            (Phase::Starting { version: version() }, Step::Restarting),
            (Phase::Failed { version: version() }, Step::Retry),
        ] {
            *lock(&updates.phase) = phase;
            assert_eq!(
                updates.offer(),
                Some(Offer {
                    version: version(),
                    step
                })
            );
            assert!(!updates.installing());
        }
        *lock(&updates.phase) = Phase::Installing;
        assert!(updates.installing());
        assert_eq!(updates.offer(), None);
    }

    #[test]
    fn a_look_that_finds_a_release_offers_its_download() {
        let updates = Updates::offering("0.3.0");
        assert_eq!(updates.checked(), Checked::Available);
        assert_eq!(updates.offer().unwrap().step, Step::Download);
    }

    #[test]
    fn a_failed_download_stays_failed_when_the_same_release_is_found_again() {
        let updates = Updates::offering("0.3.0");
        *lock(&updates.phase) = Phase::Failed {
            version: "0.3.0".into(),
        };
        updates.record(Found::Newer(Release {
            version: "0.3.0".into(),
            url: String::new(),
        }));
        assert_eq!(updates.offer().unwrap().step, Step::Retry);
        updates.record(Found::Newer(Release {
            version: "0.3.1".into(),
            url: String::new(),
        }));
        assert_eq!(
            updates.offer(),
            Some(Offer {
                version: "0.3.1".into(),
                step: Step::Download
            })
        );
    }

    #[test]
    fn nothing_restarts_before_a_download_is_ready() {
        let updates = Updates::offering("0.3.0");
        updates.restart();
        assert_eq!(updates.offer().unwrap().step, Step::Download);
    }

    #[test]
    fn a_restart_with_nothing_downloaded_fails_and_offers_the_download_again() {
        // Ready, but the package is gone: nothing to hand over.
        let updates = Updates::downloaded("0.3.0");
        updates.restart();
        assert_eq!(updates.offer().unwrap().step, Step::Retry);
        assert!(!updates.installing());
    }

    #[test]
    fn a_look_that_finds_nothing_says_so() {
        let updates = Updates::disabled();
        updates.record(Found::Nothing);
        assert_eq!(updates.checked(), Checked::UpToDate);
        assert_eq!(updates.offer(), None);
    }

    #[test]
    fn a_look_that_gets_no_answer_keeps_its_reason() {
        let updates = Updates::disabled();
        updates.record(Found::Unreachable("The update server answered 404".into()));
        assert_eq!(
            updates.checked(),
            Checked::Unreachable("The update server answered 404".into())
        );
    }

    #[test]
    fn a_copy_that_cannot_replace_itself_is_told_where_it_stands() {
        let updates = Updates::disabled();
        updates.record(Found::Elsewhere {
            version: "0.24.0".into(),
            reason: "Move the app to Applications, then open it to update.".into(),
        });
        assert!(
            matches!(updates.checked(), Checked::Elsewhere { version, .. } if version == "0.24.0")
        );
        assert_eq!(updates.offer(), None, "nothing to click");
    }

    #[test]
    fn where_checks_are_off_nothing_looks() {
        let updates = Updates::disabled();
        assert!(!updates.can_check());
        updates.check_now();
        assert_eq!(updates.checked(), Checked::Never);
    }

    fn staging(folder: &Path, name: &str, files: &[&str]) -> PathBuf {
        let path = folder.join(name);
        std::fs::create_dir_all(path.join("previous")).unwrap();
        for file in files {
            std::fs::write(path.join(file), "x").unwrap();
        }
        path
    }

    #[test]
    fn only_this_apps_finished_updates_are_cleaned_up() {
        let dir = tempfile::tempdir().unwrap();
        let later = SystemTime::now() + Duration::from_secs(3600);
        let finished = staging(
            dir.path(),
            ".tabletist-update-0123456789abcdef",
            &["handoff.json", "started", "result.txt"],
        );
        let rolled_back = staging(
            dir.path(),
            ".tabletist-update-fedcba9876543210",
            &["handoff.json", "result.txt"],
        );
        // Still under way: no mark that it ended.
        let running = staging(
            dir.path(),
            ".tabletist-update-1111111111111111",
            &["handoff.json", "ready"],
        );
        // Someone else's, and things that only look alike.
        let others = [
            staging(
                dir.path(),
                ".sample-update-4e275de2adc186bc",
                &["handoff.json", "started"],
            ),
            staging(
                dir.path(),
                ".tabletist-update-XYZ",
                &["handoff.json", "started"],
            ),
            staging(
                dir.path(),
                ".tabletist-update-AAAAAAAAAAAAAAAA",
                &["handoff.json", "started"],
            ),
            staging(
                dir.path(),
                ".tabletist-update-0123456789abcdef0",
                &["handoff.json", "started"],
            ),
            staging(
                dir.path(),
                ".tabletist-update-2222222222222222",
                &["started"],
            ),
            staging(dir.path(), "Tabletist.app", &["handoff.json", "started"]),
        ];

        let mut removed = clean_staging(dir.path(), later);
        removed.sort();
        let mut expected = vec![finished.clone(), rolled_back.clone()];
        expected.sort();
        assert_eq!(removed, expected);
        assert!(!finished.exists() && !rolled_back.exists());
        assert!(running.exists());
        for other in others {
            assert!(other.exists(), "{}", other.display());
        }
    }

    #[test]
    fn an_update_that_just_ended_is_left_for_its_helper() {
        let dir = tempfile::tempdir().unwrap();
        let fresh = staging(
            dir.path(),
            ".tabletist-update-0123456789abcdef",
            &["handoff.json", "started"],
        );
        assert!(clean_staging(dir.path(), SystemTime::now()).is_empty());
        assert!(fresh.exists());
    }
}

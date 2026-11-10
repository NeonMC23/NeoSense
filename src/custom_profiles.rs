//! Persistent, validated user-defined thermal curves.
//!
//! The store is deliberately small and boring: one versioned JSON document,
//! strict parsing, explicit validation, an advisory write lock, atomic
//! replacement and a single backup. It never chooses a default silently and
//! never marks a custom curve as hardware-applicable.
//!
//! Custom profiles are *dynamic thermal curves*, not fixed-speed presets.
//! They are usable in preview/simulation even when no hardware backend is
//! available.

use crate::policy::FanRole;
use crate::regulation::{CurvePoint, CurveSpec, validate_curve};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub const SCHEMA: &str = "neosense.custom_profiles";
pub const VERSION: u32 = 1;
pub const MIN_TARGET_PERCENT: f64 = 20.0;
pub const MAX_POINTS: usize = 32;
pub const MAX_PROFILES: usize = 64;
/// Keeps a full status response comfortably below the IPC message limit.
pub const MAX_DOCUMENT_BYTES: usize = 32 * 1024;
/// Hard ceiling before parsing untrusted JSON from disk.
pub const MAX_FILE_BYTES: u64 = 256 * 1024;
pub const MAX_TITLE: usize = 80;
pub const MAX_DESCRIPTION: usize = 500;
pub const RESERVED_NAMES: &[&str] = &["safe-auto", "quiet", "balanced", "performance", "custom"];

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn default_hysteresis() -> f64 {
    3.0
}

fn default_max_step() -> f64 {
    10.0
}

fn default_stale_after_ms() -> u64 {
    3_000
}

fn default_fan_roles() -> Vec<FanRole> {
    vec![FanRole::Cpu, FanRole::Gpu]
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomProfile {
    pub name: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub points: Vec<CurvePoint>,
    pub min_percent: f64,
    pub max_percent: f64,
    #[serde(default = "default_hysteresis")]
    pub hysteresis_c: f64,
    #[serde(default = "default_max_step")]
    pub max_step_percent: f64,
    #[serde(default = "default_stale_after_ms")]
    pub stale_after_ms: u64,
    #[serde(default = "default_fan_roles")]
    pub fan_roles: Vec<FanRole>,
}

impl CustomProfile {
    pub fn curve_spec(&self) -> CurveSpec {
        CurveSpec::from_slice(
            &self.points,
            self.min_percent,
            self.max_percent,
            self.hysteresis_c,
            self.max_step_percent,
            self.stale_after_ms,
        )
    }

    /// Validate both the generic curve invariants and the custom-profile
    /// policy limits. Errors are intended to be shown verbatim to the user.
    pub fn validate(&self) -> Result<(), String> {
        if self.name.is_empty() || self.name.len() > 64 {
            return Err("profile name must be 1-64 characters".to_string());
        }
        if !self
            .name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
            || !self
                .name
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric())
        {
            return Err(
                "profile name must start with a letter or digit and contain only a-z, 0-9, '-' or '_'"
                    .to_string(),
            );
        }
        if RESERVED_NAMES.contains(&self.name.as_str()) {
            return Err(format!(
                "profile name '{}' is reserved by a built-in profile",
                self.name
            ));
        }
        if self.title.trim().is_empty() || self.title.len() > MAX_TITLE {
            return Err(format!(
                "profile title must be 1-{MAX_TITLE} characters and not blank"
            ));
        }
        if self.description.len() > MAX_DESCRIPTION {
            return Err(format!(
                "profile description must be at most {MAX_DESCRIPTION} characters"
            ));
        }
        if self.points.len() > MAX_POINTS {
            return Err(format!("a curve may contain at most {MAX_POINTS} points"));
        }
        if !self.min_percent.is_finite() || self.min_percent < MIN_TARGET_PERCENT {
            return Err(format!(
                "minimum target must be at least {MIN_TARGET_PERCENT:.0}%"
            ));
        }
        if !self.max_percent.is_finite() || self.max_percent > 100.0 {
            return Err("maximum target must not exceed 100%".to_string());
        }
        if self.min_percent >= self.max_percent {
            return Err("minimum target must be lower than maximum target".to_string());
        }
        if !self.hysteresis_c.is_finite() || !(0.0..=20.0).contains(&self.hysteresis_c) {
            return Err("hysteresis must be between 0 and 20 °C".to_string());
        }
        if !self.max_step_percent.is_finite() || !(1.0..=50.0).contains(&self.max_step_percent) {
            return Err("maximum step must be between 1 and 50 percentage points".to_string());
        }
        if !(1_000..=60_000).contains(&self.stale_after_ms) {
            return Err("staleness window must be between 1000 and 60000 ms".to_string());
        }
        if self.fan_roles.is_empty() {
            return Err("select at least one fan role (cpu or gpu)".to_string());
        }
        for (index, role) in self.fan_roles.iter().enumerate() {
            if *role == FanRole::Unknown {
                return Err("fan role 'unknown' cannot be selected".to_string());
            }
            if self.fan_roles[..index].contains(role) {
                return Err(format!("fan role '{}' is selected more than once", role));
            }
        }
        for point in &self.points {
            if !(0.0..=150.0).contains(&point.temperature_c) {
                return Err("curve temperatures must stay between 0 and 150 °C".to_string());
            }
            if !(self.min_percent..=self.max_percent).contains(&point.percent) {
                return Err(format!(
                    "curve point percentage must be between the configured minimum ({}%) and maximum ({}%)",
                    self.min_percent, self.max_percent
                ));
            }
        }
        validate_curve(&self.curve_spec())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomProfileFile {
    pub schema: String,
    pub version: u32,
    #[serde(default)]
    pub profiles: Vec<CustomProfile>,
}

impl Default for CustomProfileFile {
    fn default() -> Self {
        Self {
            schema: SCHEMA.to_string(),
            version: VERSION,
            profiles: Vec::new(),
        }
    }
}

impl CustomProfileFile {
    pub fn find(&self, name: &str) -> Option<&CustomProfile> {
        self.profiles.iter().find(|profile| profile.name == name)
    }

    fn validate(&self) -> Result<(), String> {
        if self.schema != SCHEMA {
            return Err(format!(
                "unsupported profile document schema '{}' (expected '{SCHEMA}')",
                self.schema
            ));
        }
        if self.version != VERSION {
            return Err(format!(
                "unsupported profile document version {} (supported: {VERSION})",
                self.version
            ));
        }
        if self.profiles.len() > MAX_PROFILES {
            return Err(format!(
                "at most {MAX_PROFILES} custom profiles may be stored"
            ));
        }
        for (index, profile) in self.profiles.iter().enumerate() {
            profile.validate()?;
            if self.profiles[..index]
                .iter()
                .any(|other| other.name == profile.name)
            {
                return Err(format!("duplicate profile name '{}'", profile.name));
            }
        }
        let encoded = serde_json::to_vec(self)
            .map_err(|e| format!("profile document cannot be serialized: {e}"))?;
        if encoded.len() > MAX_DOCUMENT_BYTES {
            return Err(format!(
                "profile store is too large ({} bytes; maximum {MAX_DOCUMENT_BYTES}); reduce profile count, points or description length",
                encoded.len()
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct Loaded {
    pub file: CustomProfileFile,
    pub warnings: Vec<String>,
    pub recovered_from_backup: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomStatus {
    pub schema: String,
    pub path: String,
    pub profiles: Vec<CustomProfile>,
    pub warnings: Vec<String>,
    pub error: Option<String>,
    pub recovered_from_backup: bool,
}

impl CustomStatus {
    pub fn unavailable(path: &Path, error: impl Into<String>) -> Self {
        Self {
            schema: "neosense.custom_profiles_status/v1".to_string(),
            path: path.display().to_string(),
            profiles: Vec::new(),
            warnings: Vec::new(),
            error: Some(error.into()),
            recovered_from_backup: false,
        }
    }
}

#[derive(Debug, Clone)]
pub enum StoreError {
    Io(String),
    Invalid(String),
    Corrupt { path: PathBuf, detail: String },
    Conflict(String),
    NotFound(String),
}

impl StoreError {
    pub fn code(&self) -> &'static str {
        match self {
            StoreError::Io(_) => "config_io",
            StoreError::Invalid(_) => "config_invalid",
            StoreError::Corrupt { .. } => "config_corrupt",
            StoreError::Conflict(_) => "config_conflict",
            StoreError::NotFound(_) => "profile_not_found",
        }
    }
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Io(detail) => write!(f, "profile configuration I/O error: {detail}"),
            StoreError::Invalid(detail) => write!(f, "invalid profile configuration: {detail}"),
            StoreError::Corrupt { path, detail } => write!(
                f,
                "profile configuration at {} is corrupt: {detail}; it was left untouched",
                path.display()
            ),
            StoreError::Conflict(detail) => write!(f, "profile conflict: {detail}"),
            StoreError::NotFound(name) => write!(f, "custom profile not found: {name}"),
        }
    }
}

impl std::error::Error for StoreError {}

#[derive(Debug, Clone)]
pub struct Store {
    path: PathBuf,
}

impl Store {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn from_env_or(path: impl Into<PathBuf>) -> Self {
        Self::new(env_path().unwrap_or_else(|| path.into()))
    }

    /// Path used by the supervisor. It is writable only by the service.
    pub fn system() -> Self {
        Self::new(system_path())
    }

    /// Path used by an unprivileged CLI when no supervisor is reachable.
    pub fn client() -> Self {
        Self::new(client_path())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn backup_path(&self) -> PathBuf {
        let mut name = self
            .path
            .file_name()
            .map(|name| name.to_os_string())
            .unwrap_or_else(|| "profiles.json".into());
        name.push(".bak");
        self.path.with_file_name(name)
    }

    fn lock_path(&self) -> PathBuf {
        let mut name = self
            .path
            .file_name()
            .map(|name| name.to_os_string())
            .unwrap_or_else(|| "profiles.json".into());
        name.push(".lock");
        self.path.with_file_name(name)
    }

    fn lock(&self) -> Result<File, StoreError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| StoreError::Io(format!("create {}: {e}", parent.display())))?;
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(self.lock_path())
            .map_err(|e| StoreError::Io(format!("open write lock: {e}")))?;
        let rc =
            unsafe { libc::flock(std::os::unix::io::AsRawFd::as_raw_fd(&file), libc::LOCK_EX) };
        if rc != 0 {
            return Err(StoreError::Io(format!(
                "acquire write lock: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(file)
    }

    pub fn load(&self) -> Result<Loaded, StoreError> {
        if let Ok(metadata) = fs::metadata(&self.path) {
            if metadata.len() > MAX_FILE_BYTES {
                return self.recover_from_backup(StoreError::Corrupt {
                    path: self.path.clone(),
                    detail: format!(
                        "file is {} bytes; maximum is {MAX_FILE_BYTES}",
                        metadata.len()
                    ),
                });
            }
        }
        match fs::read_to_string(&self.path) {
            Ok(text) => match parse_document(&text) {
                Ok(file) => Ok(Loaded {
                    file,
                    warnings: Vec::new(),
                    recovered_from_backup: false,
                }),
                Err(primary) => self.recover_from_backup(primary),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut warnings = Vec::new();
                if self.backup_path().exists() {
                    warnings.push(format!(
                        "main profile file is missing; backup {} was left untouched and was not loaded",
                        self.backup_path().display()
                    ));
                }
                Ok(Loaded {
                    file: CustomProfileFile::default(),
                    warnings,
                    recovered_from_backup: false,
                })
            }
            Err(error) => Err(StoreError::Io(format!(
                "read {}: {error}",
                self.path.display()
            ))),
        }
    }

    fn recover_from_backup(&self, primary: StoreError) -> Result<Loaded, StoreError> {
        let backup = self.backup_path();
        if let Ok(metadata) = fs::metadata(&backup) {
            if metadata.len() > MAX_FILE_BYTES {
                return Err(StoreError::Corrupt {
                    path: self.path.clone(),
                    detail: format!(
                        "{primary}; backup {} is too large ({} bytes)",
                        backup.display(),
                        metadata.len()
                    ),
                });
            }
        }
        let backup_text = match fs::read_to_string(&backup) {
            Ok(text) => text,
            Err(error) => {
                return Err(StoreError::Corrupt {
                    path: self.path.clone(),
                    detail: format!(
                        "{primary}; backup {} is unavailable ({error})",
                        backup.display()
                    ),
                });
            }
        };
        match parse_document(&backup_text) {
            Ok(file) => Ok(Loaded {
                file,
                warnings: vec![format!(
                    "main profile file was invalid ({primary}); recovered {} from backup {}",
                    self.path.display(),
                    backup.display()
                )],
                recovered_from_backup: true,
            }),
            Err(backup_error) => Err(StoreError::Corrupt {
                path: self.path.clone(),
                detail: format!(
                    "{primary}; backup {} is also invalid ({backup_error})",
                    backup.display()
                ),
            }),
        }
    }

    pub fn status(&self) -> CustomStatus {
        match self.load() {
            Ok(loaded) => CustomStatus {
                schema: "neosense.custom_profiles_status/v1".to_string(),
                path: self.path.display().to_string(),
                profiles: loaded.file.profiles,
                warnings: loaded.warnings,
                error: None,
                recovered_from_backup: loaded.recovered_from_backup,
            },
            Err(error) => CustomStatus::unavailable(&self.path, error.to_string()),
        }
    }

    fn save(&self, file: &CustomProfileFile, preserve_backup: bool) -> Result<(), StoreError> {
        file.validate().map_err(StoreError::Invalid)?;
        let json = serde_json::to_string_pretty(file)
            .map_err(|e| StoreError::Io(format!("serialize profiles: {e}")))?;
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| StoreError::Io(format!("create {}: {e}", parent.display())))?;
            if !preserve_backup {
                if self.path.exists() {
                    // Replace the backup atomically as well: an interrupted
                    // copy must not leave a truncated backup as the only
                    // recovery copy.
                    let backup = self.backup_path();
                    let backup_tmp = temporary_path(&backup);
                    fs::copy(&self.path, &backup_tmp).map_err(|e| {
                        StoreError::Io(format!(
                            "back up {} to {}: {e}",
                            self.path.display(),
                            backup_tmp.display()
                        ))
                    })?;
                    if let Err(error) = fs::rename(&backup_tmp, &backup) {
                        let _ = fs::remove_file(&backup_tmp);
                        return Err(StoreError::Io(format!(
                            "replace backup {}: {error}",
                            backup.display()
                        )));
                    }
                } else {
                    // A stale backup from an explicitly removed main file
                    // must not be resurrected if the new main is later
                    // damaged.
                    let _ = fs::remove_file(self.backup_path());
                }
            }
            let tmp = temporary_path(&self.path);
            let result = (|| {
                let mut handle = OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .mode(0o600)
                    .open(&tmp)
                    .map_err(|e| StoreError::Io(format!("create {}: {e}", tmp.display())))?;
                handle
                    .write_all(json.as_bytes())
                    .map_err(|e| StoreError::Io(format!("write {}: {e}", tmp.display())))?;
                handle
                    .write_all(b"\n")
                    .map_err(|e| StoreError::Io(format!("write {}: {e}", tmp.display())))?;
                handle
                    .sync_all()
                    .map_err(|e| StoreError::Io(format!("fsync {}: {e}", tmp.display())))?;
                fs::rename(&tmp, &self.path).map_err(|e| {
                    StoreError::Io(format!(
                        "replace {} with {}: {e}",
                        self.path.display(),
                        tmp.display()
                    ))
                })?;
                if let Ok(dir) = File::open(parent) {
                    let _ = dir.sync_all();
                }
                Ok(())
            })();
            if result.is_err() {
                let _ = fs::remove_file(&tmp);
            }
            return result;
        }
        Err(StoreError::Io(
            "profile path has no parent directory".to_string(),
        ))
    }

    fn modify<F>(&self, mutate: F) -> Result<Loaded, StoreError>
    where
        F: FnOnce(&mut CustomProfileFile) -> Result<(), StoreError>,
    {
        let _lock = self.lock()?;
        let mut loaded = self.load()?;
        mutate(&mut loaded.file)?;
        loaded.file.validate().map_err(StoreError::Invalid)?;
        self.save(&loaded.file, loaded.recovered_from_backup)?;
        Ok(Loaded {
            file: loaded.file,
            warnings: loaded.warnings,
            recovered_from_backup: loaded.recovered_from_backup,
        })
    }

    pub fn create(&self, profile: CustomProfile) -> Result<Loaded, StoreError> {
        profile.validate().map_err(StoreError::Invalid)?;
        self.modify(|file| {
            if file.find(&profile.name).is_some() {
                return Err(StoreError::Conflict(format!(
                    "custom profile '{}' already exists",
                    profile.name
                )));
            }
            file.profiles.push(profile);
            Ok(())
        })
    }

    /// Replace an existing profile. The name is the identity; use
    /// [`Store::rename`] to change it explicitly.
    pub fn update(&self, profile: CustomProfile) -> Result<Loaded, StoreError> {
        profile.validate().map_err(StoreError::Invalid)?;
        let name = profile.name.clone();
        self.modify(|file| {
            let Some(existing) = file.profiles.iter_mut().find(|item| item.name == name) else {
                return Err(StoreError::NotFound(name));
            };
            *existing = profile;
            Ok(())
        })
    }

    pub fn delete(&self, name: &str) -> Result<Loaded, StoreError> {
        self.modify(|file| {
            let index = file
                .profiles
                .iter()
                .position(|item| item.name == name)
                .ok_or_else(|| StoreError::NotFound(name.to_string()))?;
            file.profiles.remove(index);
            Ok(())
        })
    }

    pub fn duplicate(&self, from: &str, to: &str) -> Result<Loaded, StoreError> {
        self.modify(|file| {
            let source = file
                .find(from)
                .cloned()
                .ok_or_else(|| StoreError::NotFound(from.to_string()))?;
            if file.find(to).is_some() {
                return Err(StoreError::Conflict(format!(
                    "custom profile '{to}' already exists"
                )));
            }
            let mut copy = source;
            copy.name = to.to_string();
            copy.validate().map_err(StoreError::Invalid)?;
            file.profiles.push(copy);
            Ok(())
        })
    }

    pub fn rename(&self, from: &str, to: &str) -> Result<Loaded, StoreError> {
        self.modify(|file| {
            if file.find(to).is_some() {
                return Err(StoreError::Conflict(format!(
                    "custom profile '{to}' already exists"
                )));
            }
            let target = file
                .profiles
                .iter_mut()
                .find(|item| item.name == from)
                .ok_or_else(|| StoreError::NotFound(from.to_string()))?;
            target.name = to.to_string();
            target.validate().map_err(StoreError::Invalid)?;
            Ok(())
        })
    }

    /// Explicit corruption recovery. The existing file (if any) is preserved
    /// with a `.corrupt-<timestamp>` suffix; nothing is silently discarded.
    pub fn reset(&self, confirm: bool) -> Result<Option<PathBuf>, StoreError> {
        if !confirm {
            return Err(StoreError::Conflict(
                "profile-store reset requires explicit confirmation".to_string(),
            ));
        }
        let _lock = self.lock()?;
        let _ = fs::remove_file(self.backup_path());
        let preserved = if self.path.exists() {
            let suffix = timestamp_suffix();
            let preserved = self.path.with_file_name(format!(
                "{}.corrupt-{suffix}",
                self.path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("profiles.json")
            ));
            fs::rename(&self.path, &preserved).map_err(|e| {
                StoreError::Io(format!(
                    "preserve corrupt {} as {}: {e}",
                    self.path.display(),
                    preserved.display()
                ))
            })?;
            Some(preserved)
        } else {
            None
        };
        self.save(&CustomProfileFile::default(), true)?;
        Ok(preserved)
    }
}

fn timestamp_suffix() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}-{}", now.as_secs(), now.subsec_nanos())
}

fn temporary_path(path: &Path) -> PathBuf {
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("profiles.json");
    path.with_file_name(format!(
        ".{name}.tmp.{}.{}.{}",
        std::process::id(),
        timestamp_suffix(),
        counter
    ))
}

fn parse_document(text: &str) -> Result<CustomProfileFile, StoreError> {
    let value: Value =
        serde_json::from_str(text).map_err(|e| StoreError::Invalid(format!("JSON parse: {e}")))?;
    if value.is_array() {
        // Legacy v0: a bare array of profile objects.
        let profiles: Vec<CustomProfile> = serde_json::from_value(value)
            .map_err(|e| StoreError::Invalid(format!("legacy profile list: {e}")))?;
        let file = CustomProfileFile {
            profiles,
            ..CustomProfileFile::default()
        };
        file.validate().map_err(StoreError::Invalid)?;
        return Ok(file);
    }
    let version = value.get("version").and_then(Value::as_u64);
    match version {
        Some(version) if version == u64::from(VERSION) => {
            let file: CustomProfileFile = serde_json::from_value(value)
                .map_err(|e| StoreError::Invalid(format!("schema: {e}")))?;
            file.validate().map_err(StoreError::Invalid)?;
            Ok(file)
        }
        Some(0) => {
            // Version 0 object: accept the same shape but rewrite it as v1 on
            // the next save. Missing optional fields use documented defaults.
            let profiles_value = value
                .get("profiles")
                .cloned()
                .ok_or_else(|| StoreError::Invalid("version 0 document lacks profiles".into()))?;
            let profiles: Vec<CustomProfile> = serde_json::from_value(profiles_value)
                .map_err(|e| StoreError::Invalid(format!("version 0 profiles: {e}")))?;
            let file = CustomProfileFile {
                profiles,
                ..CustomProfileFile::default()
            };
            file.validate().map_err(StoreError::Invalid)?;
            Ok(file)
        }
        Some(version) => Err(StoreError::Invalid(format!(
            "unsupported profile document version {version}; this build supports version {VERSION}"
        ))),
        None => Err(StoreError::Invalid(
            "profile document has no version; refusing to guess its format".to_string(),
        )),
    }
}

pub fn env_path() -> Option<PathBuf> {
    std::env::var_os("NEOSENSE_PROFILES_PATH")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

pub fn system_path() -> PathBuf {
    env_path().unwrap_or_else(|| PathBuf::from("/var/lib/neosense/profiles.json"))
}

pub fn client_path() -> PathBuf {
    if let Some(path) = env_path() {
        return path;
    }
    if let Some(config) = std::env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        return PathBuf::from(config).join("neosense/profiles.json");
    }
    if let Some(home) = std::env::var_os("HOME").filter(|value| !value.is_empty()) {
        return PathBuf::from(home).join(".config/neosense/profiles.json");
    }
    PathBuf::from("neosense-profiles.json")
}

/// Read a single profile from any store, returning an actionable error.
pub fn read_profile(store: &Store, name: &str) -> Result<CustomProfile, StoreError> {
    let loaded = store.load()?;
    loaded
        .file
        .find(name)
        .cloned()
        .ok_or_else(|| StoreError::NotFound(name.to_string()))
}

/// Create a private editor file without following an existing symlink. Used by
/// the CLI/TUI `$EDITOR` workflow for curve definitions.
pub fn editor_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "neosense-editor-{}-{}-{}.json",
        std::process::id(),
        timestamp_suffix(),
        name
    ))
}

pub fn write_private_file(path: &Path, contents: &str) -> Result<(), StoreError> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| StoreError::Io(format!("create {}: {e}", path.display())))?;
    file.write_all(contents.as_bytes())
        .map_err(|e| StoreError::Io(format!("write {}: {e}", path.display())))?;
    Ok(())
}

pub fn read_text(path: &Path) -> Result<String, StoreError> {
    let mut text = String::new();
    File::open(path)
        .and_then(|mut file| file.read_to_string(&mut text))
        .map_err(|e| StoreError::Io(format!("read {}: {e}", path.display())))?;
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn profile(name: &str) -> CustomProfile {
        CustomProfile {
            name: name.to_string(),
            title: format!("{name} title"),
            description: "test curve".to_string(),
            points: vec![
                CurvePoint {
                    temperature_c: 40.0,
                    percent: 25.0,
                },
                CurvePoint {
                    temperature_c: 70.0,
                    percent: 60.0,
                },
                CurvePoint {
                    temperature_c: 90.0,
                    percent: 100.0,
                },
            ],
            min_percent: 20.0,
            max_percent: 100.0,
            hysteresis_c: 3.0,
            max_step_percent: 10.0,
            stale_after_ms: 3_000,
            fan_roles: vec![FanRole::Cpu],
        }
    }

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("profiles.json"));
        (dir, store)
    }

    #[test]
    fn create_list_update_delete_round_trip() {
        let (_dir, store) = store();
        store.create(profile("quiet-mine")).unwrap();
        assert_eq!(store.load().unwrap().file.profiles.len(), 1);
        assert!(store.load().unwrap().file.find("quiet-mine").is_some());

        let mut updated = profile("quiet-mine");
        updated.title = "Updated".into();
        updated.max_percent = 90.0;
        updated.points[2].percent = 90.0;
        store.update(updated).unwrap();
        assert_eq!(
            store.load().unwrap().file.find("quiet-mine").unwrap().title,
            "Updated"
        );

        store.delete("quiet-mine").unwrap();
        assert!(store.load().unwrap().file.profiles.is_empty());
        assert!(matches!(
            store.delete("quiet-mine"),
            Err(StoreError::NotFound(_))
        ));
    }

    #[test]
    fn duplicate_and_rename_are_explicit() {
        let (_dir, store) = store();
        store.create(profile("curve-a")).unwrap();
        store.duplicate("curve-a", "curve-b").unwrap();
        assert_eq!(store.load().unwrap().file.profiles.len(), 2);
        store.rename("curve-b", "curve-c").unwrap();
        assert!(store.load().unwrap().file.find("curve-c").is_some());
        assert!(matches!(
            store.rename("curve-a", "curve-c"),
            Err(StoreError::Conflict(_))
        ));
    }

    #[test]
    fn invalid_values_are_rejected_with_actionable_messages() {
        let (_dir, _store) = store();
        let mut invalid = profile("Bad Name");
        assert!(invalid.validate().unwrap_err().contains("profile name"));
        invalid.name = "quiet".into();
        assert!(invalid.validate().unwrap_err().contains("reserved"));
        invalid.name = "ok".into();
        invalid.min_percent = 10.0;
        assert!(invalid.validate().unwrap_err().contains("20%"));
        invalid = profile("ok");
        invalid.points[1].temperature_c = 40.0;
        assert!(invalid.validate().unwrap_err().contains("increasing"));
        invalid = profile("ok");
        invalid.points[1].percent = 10.0;
        assert!(invalid.validate().unwrap_err().contains("minimum"));
        invalid = profile("ok");
        invalid.min_percent = 90.0;
        invalid.max_percent = 50.0;
        assert!(invalid.validate().unwrap_err().contains("lower than"));
        invalid = profile("ok");
        invalid.fan_roles = vec![];
        assert!(invalid.validate().unwrap_err().contains("at least one"));
        invalid = profile("ok");
        invalid.fan_roles = vec![FanRole::Unknown];
        assert!(invalid.validate().unwrap_err().contains("unknown"));
    }

    #[test]
    fn backup_is_created_before_replacement() {
        let (_dir, store) = store();
        store.create(profile("curve-a")).unwrap();
        assert!(!store.backup_path().exists());
        store.create(profile("curve-b")).unwrap();
        assert!(store.backup_path().exists());
        let backup = store.load().unwrap();
        assert_eq!(backup.file.profiles.len(), 2);
    }

    #[test]
    fn missing_main_is_not_silently_replaced_from_a_stale_backup() {
        let (_dir, store) = store();
        store.create(profile("curve-a")).unwrap();
        store.create(profile("curve-b")).unwrap();
        fs::remove_file(store.path()).unwrap();
        let loaded = store.load().unwrap();
        assert!(loaded.file.profiles.is_empty());
        assert!(
            loaded
                .warnings
                .iter()
                .any(|warning| warning.contains("missing"))
        );
        assert!(store.backup_path().exists());
        store.create(profile("curve-c")).unwrap();
        assert!(!store.backup_path().exists());
        assert_eq!(store.load().unwrap().file.profiles.len(), 1);
    }

    #[test]
    fn reset_removes_the_previous_generation_backup() {
        let (_dir, store) = store();
        store.create(profile("curve-a")).unwrap();
        store.create(profile("curve-b")).unwrap();
        assert!(store.backup_path().exists());
        store.reset(true).unwrap();
        assert!(!store.backup_path().exists());
        assert!(store.load().unwrap().file.profiles.is_empty());
    }

    #[test]
    fn corrupt_main_recovers_from_backup_without_overwriting_it() {
        let (_dir, store) = store();
        store.create(profile("curve-a")).unwrap();
        store.create(profile("curve-b")).unwrap();
        let good_backup = fs::read_to_string(store.backup_path()).unwrap();
        fs::write(store.path(), b"{not json").unwrap();
        let loaded = store.load().unwrap();
        assert!(loaded.recovered_from_backup);
        // The backup is the previous good generation (before the second
        // create), which is exactly what an interrupted write must recover.
        assert_eq!(loaded.file.profiles.len(), 1);
        assert!(!loaded.warnings.is_empty());
        assert_eq!(
            fs::read_to_string(store.backup_path()).unwrap(),
            good_backup
        );
        // A later mutation repairs the main file and preserves the backup.
        store.delete("curve-a").unwrap();
        assert!(parse_document(&fs::read_to_string(store.path()).unwrap()).is_ok());
        assert_eq!(
            fs::read_to_string(store.backup_path()).unwrap(),
            good_backup
        );
    }

    #[test]
    fn corrupt_main_and_backup_are_reported_and_left_untouched() {
        let (_dir, store) = store();
        store.create(profile("curve-a")).unwrap();
        store.create(profile("curve-b")).unwrap();
        fs::write(store.path(), b"broken main").unwrap();
        fs::write(store.backup_path(), b"broken backup").unwrap();
        let error = store.load().unwrap_err();
        assert_eq!(error.code(), "config_corrupt");
        assert_eq!(fs::read_to_string(store.path()).unwrap(), "broken main");
        assert_eq!(
            fs::read_to_string(store.backup_path()).unwrap(),
            "broken backup"
        );
        assert!(matches!(
            store.create(profile("curve-c")),
            Err(StoreError::Corrupt { .. })
        ));
        assert_eq!(fs::read_to_string(store.path()).unwrap(), "broken main");
    }

    #[test]
    fn explicit_reset_preserves_corrupt_input() {
        let (_dir, store) = store();
        fs::write(store.path(), b"broken").unwrap();
        let preserved = store.reset(true).unwrap().unwrap();
        assert!(preserved.exists());
        assert_eq!(fs::read(preserved).unwrap(), b"broken");
        assert!(store.load().unwrap().file.profiles.is_empty());
        assert!(matches!(store.reset(false), Err(StoreError::Conflict(_))));
    }

    #[test]
    fn legacy_array_and_version_zero_are_migrated() {
        let (_dir, store) = store();
        let legacy = serde_json::to_string(&vec![profile("legacy")]).unwrap();
        fs::write(store.path(), legacy).unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded.file.version, VERSION);
        assert_eq!(loaded.file.schema, SCHEMA);
        assert_eq!(loaded.file.profiles.len(), 1);

        let document = serde_json::json!({
            "version": 0,
            "profiles": [profile("old-object")]
        });
        fs::write(store.path(), document.to_string()).unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded.file.profiles[0].name, "old-object");
        assert_eq!(loaded.file.version, VERSION);
    }

    #[test]
    fn future_versions_and_unknown_fields_are_rejected() {
        let (_dir, store) = store();
        fs::write(
            store.path(),
            serde_json::json!({"schema": SCHEMA, "version": 99, "profiles": []}).to_string(),
        )
        .unwrap();
        assert!(store.load().unwrap_err().to_string().contains("version 99"));

        let mut value = serde_json::to_value(profile("curve-a")).unwrap();
        value["surprise"] = serde_json::json!(true);
        let document = serde_json::json!({
            "schema": SCHEMA,
            "version": VERSION,
            "profiles": [value]
        });
        fs::write(store.path(), document.to_string()).unwrap();
        assert!(
            store
                .load()
                .unwrap_err()
                .to_string()
                .contains("unknown field")
        );
    }

    #[test]
    fn concurrent_creates_are_serialized_by_the_store_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profiles.json");
        let handles: Vec<_> = (0..8)
            .map(|index| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let store = Store::new(path);
                    store.create(profile(&format!("curve-{index}"))).unwrap();
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        let store = Store::new(path);
        let loaded = store.load().unwrap();
        assert_eq!(loaded.file.profiles.len(), 8);
    }

    #[test]
    fn atomic_write_leaves_no_temporary_files() {
        let (dir, store) = store();
        store.create(profile("curve-a")).unwrap();
        let names: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(names.iter().all(|name| !name.contains(".tmp")), "{names:?}");
        assert!(names.contains(&"profiles.json".to_string()));
    }

    #[test]
    fn oversized_input_is_rejected_without_parsing() {
        let (_dir, store) = store();
        fs::write(store.path(), vec![b' '; (MAX_FILE_BYTES + 1) as usize]).unwrap();
        let error = store.load().unwrap_err();
        assert_eq!(error.code(), "config_corrupt");
        assert_eq!(
            fs::metadata(store.path()).unwrap().len(),
            MAX_FILE_BYTES + 1
        );
    }

    #[test]
    fn status_reports_errors_without_fabricating_profiles() {
        let (_dir, store) = store();
        fs::write(store.path(), b"broken").unwrap();
        let status = store.status();
        assert!(status.error.is_some());
        assert!(status.profiles.is_empty());
        assert_eq!(status.schema, "neosense.custom_profiles_status/v1");
        assert_eq!(store.load().unwrap_err().code(), "config_corrupt");
    }

    #[test]
    fn oversized_store_is_rejected_before_it_can_break_ipc_status() {
        let mut file = CustomProfileFile::default();
        for index in 0..MAX_PROFILES {
            let mut item = profile(&format!("curve-{index}"));
            item.description = "d".repeat(MAX_DESCRIPTION);
            file.profiles.push(item);
        }
        let error = file.validate().unwrap_err();
        assert!(error.contains("too large"), "{error}");
    }

    #[test]
    fn profile_file_rejects_duplicate_names() {
        let mut file = CustomProfileFile::default();
        file.profiles.push(profile("same"));
        file.profiles.push(profile("same"));
        assert!(file.validate().unwrap_err().contains("duplicate"));
    }

    #[test]
    fn arc_store_can_be_shared_across_operations() {
        let (_dir, store) = store();
        let store = Arc::new(store);
        let cloned = Arc::clone(&store);
        cloned.create(profile("shared")).unwrap();
        assert!(store.load().unwrap().file.find("shared").is_some());
    }
}

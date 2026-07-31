use std::cmp::Reverse;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result, anyhow, bail};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheScan {
    pub codex_root: PathBuf,
    pub standalone_root: PathBuf,
    pub releases_dir: PathBuf,
    pub current_target: Option<PathBuf>,
    pub current_version: Option<String>,
    pub releases: Vec<ReleaseInfo>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseInfo {
    pub display_version: String,
    pub directory_name: String,
    pub path: PathBuf,
    pub size: u64,
    pub active: bool,
    version_key: VersionKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepPolicy {
    Current,
    CurrentPrevious,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletionPlan {
    pub keep_policy: KeepPolicy,
    pub releases_to_remove: Vec<ReleaseInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionResult;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationReport {
    pub checks: Vec<VerificationCheck>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationCheck {
    pub status: VerificationStatus,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerificationStatus {
    Pass,
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct VersionKey {
    major: u64,
    minor: u64,
    patch: u64,
}

impl CacheScan {
    pub fn total_size(&self) -> u64 {
        self.releases.iter().map(|release| release.size).sum()
    }

    pub fn current_size(&self) -> u64 {
        self.releases
            .iter()
            .filter(|release| release.active)
            .map(|release| release.size)
            .sum()
    }

    pub fn reclaim_if_only_current_kept(&self) -> u64 {
        self.total_size().saturating_sub(self.current_size())
    }

    pub fn reclaim_if_current_and_previous_kept(&self) -> u64 {
        let keep = self
            .current_release()
            .into_iter()
            .chain(self.previous_release())
            .map(|release| release.size)
            .sum::<u64>();

        self.total_size().saturating_sub(keep)
    }

    pub fn current_release(&self) -> Option<&ReleaseInfo> {
        self.releases.iter().find(|release| release.active)
    }

    pub fn previous_release(&self) -> Option<&ReleaseInfo> {
        self.releases.iter().find(|release| !release.active)
    }
}

impl DeletionPlan {
    pub fn reclaimed_bytes(&self) -> u64 {
        self.releases_to_remove
            .iter()
            .map(|release| release.size)
            .sum()
    }
}

impl VerificationReport {
    pub fn error_count(&self) -> usize {
        self.checks
            .iter()
            .filter(|check| check.status == VerificationStatus::Error)
            .count()
    }

    pub fn warning_count(&self) -> usize {
        self.checks
            .iter()
            .filter(|check| check.status == VerificationStatus::Warning)
            .count()
    }

    pub fn pass_count(&self) -> usize {
        self.checks
            .iter()
            .filter(|check| check.status == VerificationStatus::Pass)
            .count()
    }
}

impl KeepPolicy {
    fn label(self) -> &'static str {
        match self {
            KeepPolicy::Current => "current",
            KeepPolicy::CurrentPrevious => "current,previous",
        }
    }
}

impl VerificationStatus {
    fn label(self) -> &'static str {
        match self {
            VerificationStatus::Pass => "PASS",
            VerificationStatus::Warning => "WARNING",
            VerificationStatus::Error => "ERROR",
        }
    }
}

impl FromStr for KeepPolicy {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "current" => Ok(Self::Current),
            "current,previous" => Ok(Self::CurrentPrevious),
            _ => bail!("keep must be one of: current, current,previous"),
        }
    }
}

pub fn scan_default() -> Result<CacheScan> {
    let codex_root = codex_root()?;
    scan_at(&codex_root)
}

pub fn scan_releases_default() -> Result<Vec<ReleaseInfo>> {
    Ok(scan_default()?.releases)
}

pub fn scan_releases_at(codex_root: &Path) -> Result<Vec<ReleaseInfo>> {
    Ok(scan_at(codex_root)?.releases)
}

pub fn scan_at(codex_root: &Path) -> Result<CacheScan> {
    let standalone_root = codex_root.join("packages").join("standalone");
    let releases_dir = standalone_root.join("releases");

    if !codex_root.exists() {
        bail!("Codex root was not found at {}", codex_root.display());
    }

    if !releases_dir.is_dir() {
        bail!(
            "Codex standalone release cache was not found at {}",
            releases_dir.display()
        );
    }

    let current_target = read_current_target(&standalone_root)?;
    let current_name = current_target
        .as_deref()
        .and_then(|target| target.file_name())
        .map(|name| name.to_string_lossy().into_owned());
    let current_version = current_name
        .as_deref()
        .and_then(|name| parse_version(name).ok());

    let (releases, warnings) = discover_releases(&releases_dir, current_target.as_deref())?;

    Ok(CacheScan {
        codex_root: codex_root.to_path_buf(),
        standalone_root,
        releases_dir,
        current_target,
        current_version,
        releases,
        warnings,
    })
}

pub fn render_scan(scan: &CacheScan) -> String {
    let mut output = String::new();
    output.push_str("Codex standalone cache scan\n");
    output.push_str(&format!("Codex root:      {}\n", scan.codex_root.display()));
    output.push_str(&format!(
        "Standalone root: {}\n",
        scan.standalone_root.display()
    ));
    output.push_str(&format!(
        "Releases dir:    {}\n",
        scan.releases_dir.display()
    ));
    output.push_str(&format!(
        "Current target:  {}\n",
        scan.current_target
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "not found".to_string())
    ));
    output.push_str(&format!(
        "Current version: {}\n\n",
        scan.current_version.as_deref().unwrap_or("unknown")
    ));

    output.push_str("Installed releases:\n");
    if scan.releases.is_empty() {
        output.push_str("  none\n");
    } else {
        for release in &scan.releases {
            output.push_str(&format!(
                "  {:<12} {:>10}  {}  {}\n",
                release.display_version,
                format_bytes(release.size),
                if release.active {
                    "active  "
                } else {
                    "inactive"
                },
                release.path.display()
            ));
        }
    }

    append_warnings(&mut output, &scan.warnings);
    output
}

pub fn render_report(scan: &CacheScan) -> String {
    let mut output = String::new();
    output.push_str("Codex standalone cache report\n");
    output.push_str(&format!(
        "Current version: {}\n",
        scan.current_version.as_deref().unwrap_or("unknown")
    ));
    output.push_str(&format!(
        "Installed versions: {}\n",
        installed_versions(scan)
    ));
    output.push_str(&format!(
        "Total cache size: {}\n",
        format_bytes(scan.total_size())
    ));
    output.push_str(&format!(
        "Potential reclaim, keeping current only: {}\n",
        format_bytes(scan.reclaim_if_only_current_kept())
    ));
    output.push_str(&format!(
        "Potential reclaim, keeping current + previous: {}\n\n",
        format_bytes(scan.reclaim_if_current_and_previous_kept())
    ));

    output.push_str("Largest releases:\n");
    let mut largest = scan.releases.iter().collect::<Vec<_>>();
    largest.sort_by_key(|release| Reverse(release.size));
    for release in largest.into_iter().take(5) {
        output.push_str(&format!(
            "  {:<12} {:>10}  {}\n",
            release.display_version,
            format_bytes(release.size),
            release.path.display()
        ));
    }
    if scan.releases.is_empty() {
        output.push_str("  none\n");
    }

    append_warnings(&mut output, &scan.warnings);
    output
}

pub fn render_list(scan: &CacheScan) -> String {
    let mut output = String::new();
    output.push_str("Version     Active  Size\n");
    for release in &scan.releases {
        output.push_str(&format!(
            "{:<11} {:<7} {}\n",
            release.display_version,
            if release.active { "*" } else { "" },
            format_bytes(release.size)
        ));
    }
    if scan.releases.is_empty() {
        output.push_str("none\n");
    }

    append_warnings(&mut output, &scan.warnings);
    output
}

pub fn plan_deletions(releases: &[ReleaseInfo], keep_policy: KeepPolicy) -> Result<DeletionPlan> {
    let active = releases
        .iter()
        .find(|release| release.active)
        .ok_or_else(|| {
            anyhow!(
                "active Codex release could not be determined; refusing to produce a deletion plan"
            )
        })?;
    let previous = (keep_policy == KeepPolicy::CurrentPrevious)
        .then(|| releases.iter().find(|release| !release.active))
        .flatten();

    let releases_to_remove = releases
        .iter()
        .filter(|release| release.path != active.path)
        .filter(|release| previous.is_none_or(|previous| release.path != previous.path))
        .cloned()
        .collect();

    Ok(DeletionPlan {
        keep_policy,
        releases_to_remove,
    })
}

pub fn execute_deletion_plan(_plan: &DeletionPlan) -> Result<ExecutionResult> {
    bail!("Deletion execution is not yet implemented.")
}

pub fn render_clean_dry_run(plan: &DeletionPlan) -> String {
    let mut output = String::new();
    output.push_str("Codex standalone cache clean dry run\n");
    output.push_str(&format!("Keep policy: {}\n", plan.keep_policy.label()));
    output.push_str("Would remove:\n");
    if plan.releases_to_remove.is_empty() {
        output.push_str("  none\n");
    } else {
        for release in &plan.releases_to_remove {
            output.push_str(&format!("  {}\n", release.path.display()));
        }
    }
    output.push_str(&format!("Reclaimed bytes: {}\n", plan.reclaimed_bytes()));
    output.push_str(&format!(
        "Reclaimed size: {}\n",
        format_bytes(plan.reclaimed_bytes())
    ));
    output.push_str("Dry run: no files were deleted.\n");
    output
}

pub fn verify_default() -> Result<VerificationReport> {
    let codex_root = codex_root()?;
    Ok(verify_at(&codex_root))
}

pub fn verify_at(codex_root: &Path) -> VerificationReport {
    let standalone_root = codex_root.join("packages").join("standalone");
    let releases_dir = standalone_root.join("releases");
    let current = standalone_root.join("current");
    let mut checks = Vec::new();

    check(
        &mut checks,
        releases_dir.is_dir(),
        format!("releases directory exists: {}", releases_dir.display()),
        VerificationStatus::Error,
        format!("releases directory is missing: {}", releases_dir.display()),
    );

    let releases_readable = match fs::read_dir(&releases_dir) {
        Ok(_) => {
            pass(
                &mut checks,
                format!("releases directory is readable: {}", releases_dir.display()),
            );
            true
        }
        Err(error) => {
            error_check(
                &mut checks,
                format!(
                    "releases directory is not readable: {} ({error})",
                    releases_dir.display()
                ),
            );
            false
        }
    };

    warn_about_unexpected_layout(&mut checks, &standalone_root);

    let current_target = match fs::symlink_metadata(&current) {
        Ok(metadata) => {
            pass(
                &mut checks,
                format!("current symlink exists: {}", current.display()),
            );

            if metadata.file_type().is_symlink() {
                pass(&mut checks, "current is a symlink".to_string());
                match read_current_target(&standalone_root) {
                    Ok(Some(target)) => {
                        if target.exists() {
                            pass(
                                &mut checks,
                                format!("current target exists: {}", target.display()),
                            );
                        } else {
                            error_check(
                                &mut checks,
                                format!("current symlink is dangling: {}", target.display()),
                            );
                        }

                        check(
                            &mut checks,
                            path_is_inside(&target, &releases_dir),
                            format!(
                                "current target resides inside releases/: {}",
                                target.display()
                            ),
                            VerificationStatus::Error,
                            format!("current target is outside releases/: {}", target.display()),
                        );
                        Some(target)
                    }
                    Ok(None) => None,
                    Err(error) => {
                        error_check(
                            &mut checks,
                            format!("failed to read current symlink: {error}"),
                        );
                        None
                    }
                }
            } else {
                error_check(
                    &mut checks,
                    "current exists but is not a symlink".to_string(),
                );
                None
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            error_check(
                &mut checks,
                format!("current symlink is missing: {}", current.display()),
            );
            None
        }
        Err(error) => {
            error_check(
                &mut checks,
                format!(
                    "failed to inspect current symlink: {} ({error})",
                    current.display()
                ),
            );
            None
        }
    };

    if releases_readable {
        match discover_releases(&releases_dir, current_target.as_deref()) {
            Ok((releases, warnings)) => {
                for warning in warnings {
                    warning_check(&mut checks, warning);
                }

                if current_target.is_some() {
                    let active_count = releases.iter().filter(|release| release.active).count();
                    check(
                        &mut checks,
                        active_count == 1,
                        "active release appears exactly once".to_string(),
                        VerificationStatus::Error,
                        format!("active release appears {active_count} times"),
                    );
                } else {
                    error_check(
                        &mut checks,
                        "active release cannot be checked without a readable current symlink"
                            .to_string(),
                    );
                }
            }
            Err(error) => error_check(
                &mut checks,
                format!("failed to inspect release directories: {error}"),
            ),
        }
    }

    VerificationReport { checks }
}

pub fn render_verify(report: &VerificationReport) -> String {
    let mut output = String::new();
    output.push_str("Codex standalone cache verification\n");
    for check in &report.checks {
        output.push_str(&format!("{:<7} {}\n", check.status.label(), check.message));
    }
    output.push_str(&format!(
        "\nSummary: {} PASS, {} WARNING, {} ERROR\n",
        report.pass_count(),
        report.warning_count(),
        report.error_count()
    ));
    output
}

fn discover_releases(
    releases_dir: &Path,
    current_target: Option<&Path>,
) -> Result<(Vec<ReleaseInfo>, Vec<String>)> {
    let mut warnings = Vec::new();
    let mut releases = Vec::new();

    for entry in fs::read_dir(releases_dir)
        .with_context(|| format!("failed to read {}", releases_dir.display()))?
    {
        let entry = entry.with_context(|| format!("failed to read {}", releases_dir.display()))?;
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to inspect {}", entry.path().display()))?;
        if !file_type.is_dir() {
            warnings.push(format!(
                "unexpected non-directory entry in releases/: {}",
                entry.path().display()
            ));
            continue;
        }

        let directory_name = entry.file_name().to_string_lossy().into_owned();
        let Ok((version, version_key)) = parse_release_name(&directory_name) else {
            warnings.push(format!(
                "ignored release directory with unrecognized version: {}",
                entry.path().display()
            ));
            continue;
        };

        let path = entry.path();
        let active = current_target.is_some_and(|target| same_path(target, &path));
        let size =
            directory_size(&path).with_context(|| format!("failed to size {}", path.display()))?;

        releases.push(ReleaseInfo {
            display_version: version,
            directory_name,
            path,
            size,
            active,
            version_key,
        });
    }

    releases.sort_by(|left, right| {
        right
            .version_key
            .cmp(&left.version_key)
            .then_with(|| right.directory_name.cmp(&left.directory_name))
            .then_with(|| right.path.cmp(&left.path))
    });

    Ok((releases, warnings))
}

fn warn_about_unexpected_layout(checks: &mut Vec<VerificationCheck>, standalone_root: &Path) {
    let entries = match fs::read_dir(standalone_root) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name != "current" && name != "releases" {
            warning_check(
                checks,
                format!(
                    "unexpected entry under standalone root: {}",
                    entry.path().display()
                ),
            );
        }
    }
}

fn path_is_inside(path: &Path, directory: &Path) -> bool {
    match (fs::canonicalize(path), fs::canonicalize(directory)) {
        (Ok(path), Ok(directory)) => path.starts_with(directory),
        _ => path.starts_with(directory),
    }
}

fn check(
    checks: &mut Vec<VerificationCheck>,
    passed: bool,
    pass_message: String,
    failure_status: VerificationStatus,
    failure_message: String,
) {
    if passed {
        pass(checks, pass_message);
    } else {
        checks.push(VerificationCheck {
            status: failure_status,
            message: failure_message,
        });
    }
}

fn pass(checks: &mut Vec<VerificationCheck>, message: String) {
    checks.push(VerificationCheck {
        status: VerificationStatus::Pass,
        message,
    });
}

fn warning_check(checks: &mut Vec<VerificationCheck>, message: String) {
    checks.push(VerificationCheck {
        status: VerificationStatus::Warning,
        message,
    });
}

fn error_check(checks: &mut Vec<VerificationCheck>, message: String) {
    checks.push(VerificationCheck {
        status: VerificationStatus::Error,
        message,
    });
}

pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = UNITS[0];

    for next_unit in UNITS.iter().skip(1) {
        if value < 1024.0 {
            break;
        }
        value /= 1024.0;
        unit = next_unit;
    }

    if unit == "B" {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {unit}")
    }
}

fn codex_root() -> Result<PathBuf> {
    if let Some(root) = env::var_os("CODEX_HOME") {
        return Ok(PathBuf::from(root));
    }

    let home = env::var_os("HOME").ok_or_else(|| {
        anyhow!("HOME is not set; set CODEX_HOME or HOME so the Codex root can be detected")
    })?;
    Ok(PathBuf::from(home).join(".codex"))
}

fn installed_versions(scan: &CacheScan) -> String {
    if scan.releases.is_empty() {
        return "none".to_string();
    }

    scan.releases
        .iter()
        .map(|release| release.display_version.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn read_current_target(standalone_root: &Path) -> Result<Option<PathBuf>> {
    let current = standalone_root.join("current");
    let target = match fs::read_link(&current) {
        Ok(target) => target,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", current.display()));
        }
    };

    Ok(Some(if target.is_absolute() {
        target
    } else {
        standalone_root.join(target)
    }))
}

fn same_path(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }

    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn parse_release_name(name: &str) -> Result<(String, VersionKey)> {
    let version = parse_version(name)?;
    let version_key = parse_version_key(&version)?;
    Ok((version, version_key))
}

fn parse_version(name: &str) -> Result<String> {
    let version = name.split_once('-').map_or(name, |(version, _)| version);
    parse_version_key(version)?;
    Ok(version.to_string())
}

fn parse_version_key(version: &str) -> Result<VersionKey> {
    let mut parts = version.split('.');
    let major = parse_version_part(parts.next(), version)?;
    let minor = parse_version_part(parts.next(), version)?;
    let patch = parse_version_part(parts.next(), version)?;

    if parts.next().is_some() {
        bail!("invalid release version: {version}");
    }

    Ok(VersionKey {
        major,
        minor,
        patch,
    })
}

fn parse_version_part(part: Option<&str>, version: &str) -> Result<u64> {
    let Some(part) = part else {
        bail!("invalid release version: {version}");
    };
    if part.is_empty() || !part.chars().all(|ch| ch.is_ascii_digit()) {
        bail!("invalid release version: {version}");
    }
    part.parse()
        .with_context(|| format!("invalid release version: {version}"))
}

fn directory_size(path: &Path) -> Result<u64> {
    let mut total = 0_u64;
    for entry in fs::read_dir(path).with_context(|| format!("failed to read {}", path.display()))? {
        let entry = entry.with_context(|| format!("failed to read {}", path.display()))?;
        let metadata = fs::symlink_metadata(entry.path())
            .with_context(|| format!("failed to inspect {}", entry.path().display()))?;
        if metadata.is_dir() {
            total = total.saturating_add(directory_size(&entry.path())?);
        } else {
            total = total.saturating_add(metadata.len());
        }
    }
    Ok(total)
}

fn append_warnings(output: &mut String, warnings: &[String]) {
    if warnings.is_empty() {
        return;
    }

    output.push_str("\nWarnings:\n");
    for warning in warnings {
        output.push_str(&format!("  {warning}\n"));
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::path::Path;

    use super::{
        KeepPolicy, VerificationStatus, execute_deletion_plan, format_bytes, plan_deletions,
        render_clean_dry_run, render_list, render_report, render_scan, render_verify, scan_at,
        verify_at,
    };

    #[test]
    fn scan_detects_releases_and_active_current() {
        let directory = tempfile::tempdir().unwrap();
        write_release(directory.path(), "0.144.5-x86_64-unknown-linux-musl", 20);
        write_release(directory.path(), "0.145.0-x86_64-unknown-linux-musl", 30);
        symlink(
            "releases/0.145.0-x86_64-unknown-linux-musl",
            directory
                .path()
                .join("packages")
                .join("standalone")
                .join("current"),
        )
        .unwrap();

        let scan = scan_at(directory.path()).unwrap();

        assert_eq!(scan.current_version.as_deref(), Some("0.145.0"));
        assert_eq!(scan.releases.len(), 2);
        assert_eq!(scan.releases[0].display_version, "0.145.0");
        assert!(scan.releases[0].active);
        assert_eq!(scan.releases[1].display_version, "0.144.5");
        assert!(!scan.releases[1].active);
        assert_eq!(scan.total_size(), 50);
        assert_eq!(scan.reclaim_if_only_current_kept(), 20);
    }

    #[test]
    fn active_detection_uses_full_release_path_not_display_version() {
        let directory = tempfile::tempdir().unwrap();
        write_release(directory.path(), "0.145.0-aarch64-apple-darwin", 10);
        write_release(directory.path(), "0.145.0-x86_64-unknown-linux-musl", 20);
        symlink(
            "releases/0.145.0-x86_64-unknown-linux-musl",
            directory
                .path()
                .join("packages")
                .join("standalone")
                .join("current"),
        )
        .unwrap();

        let scan = scan_at(directory.path()).unwrap();

        assert_eq!(scan.releases.len(), 2);
        assert_eq!(
            scan.current_release().unwrap().directory_name,
            "0.145.0-x86_64-unknown-linux-musl"
        );
        assert_eq!(
            scan.releases
                .iter()
                .filter(|release| release.display_version == "0.145.0")
                .count(),
            2
        );
    }

    #[test]
    fn report_keeps_current_and_previous_for_reclaim_calculation() {
        let directory = tempfile::tempdir().unwrap();
        write_release(directory.path(), "0.144.0-x86_64-unknown-linux-musl", 10);
        write_release(directory.path(), "0.144.5-x86_64-unknown-linux-musl", 20);
        write_release(directory.path(), "0.145.0-x86_64-unknown-linux-musl", 30);
        symlink(
            "releases/0.145.0-x86_64-unknown-linux-musl",
            directory
                .path()
                .join("packages")
                .join("standalone")
                .join("current"),
        )
        .unwrap();

        let scan = scan_at(directory.path()).unwrap();

        assert_eq!(scan.reclaim_if_current_and_previous_kept(), 10);
        let report = render_report(&scan);
        assert!(report.contains("Potential reclaim, keeping current + previous: 10 B"));
        assert!(report.contains("Installed versions: 0.145.0, 0.144.5, 0.144.0"));
    }

    #[test]
    fn list_renders_compact_table() {
        let directory = tempfile::tempdir().unwrap();
        write_release(directory.path(), "0.144.5-x86_64-unknown-linux-musl", 20);
        write_release(directory.path(), "0.145.0-x86_64-unknown-linux-musl", 30);
        symlink(
            "releases/0.145.0-x86_64-unknown-linux-musl",
            directory
                .path()
                .join("packages")
                .join("standalone")
                .join("current"),
        )
        .unwrap();

        let output = render_list(&scan_at(directory.path()).unwrap());

        assert!(output.starts_with("Version     Active  Size\n"));
        assert!(output.contains("0.145.0     *       30 B\n"));
        assert!(output.contains("0.144.5             20 B\n"));
    }

    #[test]
    fn scan_warns_about_unrecognized_release_directories() {
        let directory = tempfile::tempdir().unwrap();
        write_release(directory.path(), "0.145.0-x86_64-unknown-linux-musl", 30);
        fs::create_dir_all(
            directory
                .path()
                .join("packages")
                .join("standalone")
                .join("releases")
                .join("latest"),
        )
        .unwrap();

        let scan = scan_at(directory.path()).unwrap();

        assert_eq!(scan.releases.len(), 1);
        assert_eq!(scan.warnings.len(), 1);
        assert!(scan.warnings[0].contains("latest"));
    }

    #[test]
    fn keep_current_plans_all_inactive_releases() {
        let directory = tempfile::tempdir().unwrap();
        write_release(directory.path(), "0.144.0-x86_64-unknown-linux-musl", 10);
        write_release(directory.path(), "0.144.5-x86_64-unknown-linux-musl", 20);
        write_release(directory.path(), "0.145.0-x86_64-unknown-linux-musl", 30);
        symlink(
            "releases/0.145.0-x86_64-unknown-linux-musl",
            directory
                .path()
                .join("packages")
                .join("standalone")
                .join("current"),
        )
        .unwrap();

        let scan = scan_at(directory.path()).unwrap();
        let plan = plan_deletions(&scan.releases, KeepPolicy::Current).unwrap();

        assert_eq!(plan.reclaimed_bytes(), 30);
        assert_eq!(
            release_names(&plan.releases_to_remove),
            vec![
                "0.144.5-x86_64-unknown-linux-musl",
                "0.144.0-x86_64-unknown-linux-musl"
            ]
        );
    }

    #[test]
    fn keep_current_previous_keeps_newest_inactive_release() {
        let directory = tempfile::tempdir().unwrap();
        write_release(directory.path(), "0.144.0-x86_64-unknown-linux-musl", 10);
        write_release(directory.path(), "0.144.5-x86_64-unknown-linux-musl", 20);
        write_release(directory.path(), "0.145.0-x86_64-unknown-linux-musl", 30);
        symlink(
            "releases/0.145.0-x86_64-unknown-linux-musl",
            directory
                .path()
                .join("packages")
                .join("standalone")
                .join("current"),
        )
        .unwrap();

        let scan = scan_at(directory.path()).unwrap();
        let plan = plan_deletions(&scan.releases, KeepPolicy::CurrentPrevious).unwrap();

        assert_eq!(plan.reclaimed_bytes(), 10);
        assert_eq!(
            release_names(&plan.releases_to_remove),
            vec!["0.144.0-x86_64-unknown-linux-musl"]
        );
    }

    #[test]
    fn missing_active_symlink_refuses_deletion_plan() {
        let directory = tempfile::tempdir().unwrap();
        write_release(directory.path(), "0.144.5-x86_64-unknown-linux-musl", 20);
        write_release(directory.path(), "0.145.0-x86_64-unknown-linux-musl", 30);

        let scan = scan_at(directory.path()).unwrap();
        let error = plan_deletions(&scan.releases, KeepPolicy::Current)
            .unwrap_err()
            .to_string();

        assert!(error.contains("active Codex release could not be determined"));
    }

    #[test]
    fn dry_run_output_lists_directories_and_reclaim_sizes() {
        let directory = tempfile::tempdir().unwrap();
        write_release(directory.path(), "0.144.5-x86_64-unknown-linux-musl", 2048);
        write_release(directory.path(), "0.145.0-x86_64-unknown-linux-musl", 1024);
        symlink(
            "releases/0.145.0-x86_64-unknown-linux-musl",
            directory
                .path()
                .join("packages")
                .join("standalone")
                .join("current"),
        )
        .unwrap();

        let scan = scan_at(directory.path()).unwrap();
        let plan = plan_deletions(&scan.releases, KeepPolicy::Current).unwrap();
        let output = render_clean_dry_run(&plan);

        assert!(output.contains("Keep policy: current\n"));
        assert!(output.contains("0.144.5-x86_64-unknown-linux-musl"));
        assert!(output.contains("Reclaimed bytes: 2048\n"));
        assert!(output.contains("Reclaimed size: 2.0 KiB\n"));
        assert!(output.contains("Dry run: no files were deleted.\n"));
    }

    #[test]
    fn execute_deletion_plan_is_stubbed() {
        let directory = tempfile::tempdir().unwrap();
        write_release(directory.path(), "0.144.5-x86_64-unknown-linux-musl", 20);
        write_release(directory.path(), "0.145.0-x86_64-unknown-linux-musl", 30);
        symlink(
            "releases/0.145.0-x86_64-unknown-linux-musl",
            directory
                .path()
                .join("packages")
                .join("standalone")
                .join("current"),
        )
        .unwrap();

        let scan = scan_at(directory.path()).unwrap();
        let plan = plan_deletions(&scan.releases, KeepPolicy::Current).unwrap();
        let error = execute_deletion_plan(&plan).unwrap_err().to_string();

        assert_eq!(error, "Deletion execution is not yet implemented.");
    }

    #[test]
    fn verify_valid_installation_reports_success_summary() {
        let directory = tempfile::tempdir().unwrap();
        write_release(directory.path(), "0.145.0-x86_64-unknown-linux-musl", 30);
        symlink(
            "releases/0.145.0-x86_64-unknown-linux-musl",
            directory
                .path()
                .join("packages")
                .join("standalone")
                .join("current"),
        )
        .unwrap();

        let report = verify_at(directory.path());
        let output = render_verify(&report);

        assert_eq!(report.error_count(), 0);
        assert_eq!(report.warning_count(), 0);
        assert!(output.contains("PASS    active release appears exactly once"));
        assert!(output.contains("Summary:"));
        assert!(output.contains("0 ERROR"));
    }

    #[test]
    fn verify_missing_current_reports_error_summary() {
        let directory = tempfile::tempdir().unwrap();
        write_release(directory.path(), "0.145.0-x86_64-unknown-linux-musl", 30);

        let report = verify_at(directory.path());

        assert!(has_error(&report, "current symlink is missing"));
        assert!(has_error(&report, "active release cannot be checked"));
        assert!(render_verify(&report).contains("ERROR"));
    }

    #[test]
    fn verify_dangling_current_reports_error() {
        let directory = tempfile::tempdir().unwrap();
        write_release(directory.path(), "0.145.0-x86_64-unknown-linux-musl", 30);
        symlink(
            "releases/9.999.9-x86_64-unknown-linux-musl",
            directory
                .path()
                .join("packages")
                .join("standalone")
                .join("current"),
        )
        .unwrap();

        let report = verify_at(directory.path());

        assert!(has_error(&report, "current symlink is dangling"));
        assert!(has_error(&report, "active release appears 0 times"));
    }

    #[test]
    fn verify_current_outside_releases_reports_error() {
        let directory = tempfile::tempdir().unwrap();
        write_release(directory.path(), "0.145.0-x86_64-unknown-linux-musl", 30);
        let outside = directory.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        symlink(
            &outside,
            directory
                .path()
                .join("packages")
                .join("standalone")
                .join("current"),
        )
        .unwrap();

        let report = verify_at(directory.path());

        assert!(has_error(&report, "current target is outside releases/"));
        assert!(has_error(&report, "active release appears 0 times"));
    }

    #[test]
    fn verify_malformed_release_names_are_warnings() {
        let directory = tempfile::tempdir().unwrap();
        write_release(directory.path(), "0.145.0-x86_64-unknown-linux-musl", 30);
        fs::create_dir_all(
            directory
                .path()
                .join("packages")
                .join("standalone")
                .join("releases")
                .join("latest"),
        )
        .unwrap();
        symlink(
            "releases/0.145.0-x86_64-unknown-linux-musl",
            directory
                .path()
                .join("packages")
                .join("standalone")
                .join("current"),
        )
        .unwrap();

        let report = verify_at(directory.path());

        assert!(has_warning(&report, "unrecognized version"));
        assert_eq!(report.error_count(), 0);
    }

    #[test]
    fn verify_missing_releases_directory_reports_error() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir_all(directory.path().join("packages").join("standalone")).unwrap();

        let report = verify_at(directory.path());

        assert!(has_error(&report, "releases directory is missing"));
        assert!(has_error(&report, "releases directory is not readable"));
    }

    #[test]
    fn verify_unexpected_layout_reports_warning_only() {
        let directory = tempfile::tempdir().unwrap();
        write_release(directory.path(), "0.145.0-x86_64-unknown-linux-musl", 30);
        fs::create_dir_all(
            directory
                .path()
                .join("packages")
                .join("standalone")
                .join("scratch"),
        )
        .unwrap();
        symlink(
            "releases/0.145.0-x86_64-unknown-linux-musl",
            directory
                .path()
                .join("packages")
                .join("standalone")
                .join("current"),
        )
        .unwrap();

        let report = verify_at(directory.path());

        assert!(has_warning(
            &report,
            "unexpected entry under standalone root"
        ));
        assert_eq!(report.error_count(), 0);
    }

    #[test]
    fn render_scan_includes_evidence_paths() {
        let directory = tempfile::tempdir().unwrap();
        write_release(directory.path(), "0.145.0-x86_64-unknown-linux-musl", 30);

        let scan = scan_at(directory.path()).unwrap();
        let output = render_scan(&scan);

        assert!(output.contains("Codex root:"));
        assert!(output.contains("Current target:  not found"));
        assert!(output.contains("0.145.0"));
        assert!(output.contains("inactive"));
    }

    #[test]
    fn missing_cache_returns_contextual_error() {
        let directory = tempfile::tempdir().unwrap();

        let error = scan_at(directory.path()).unwrap_err().to_string();

        assert!(error.contains("Codex standalone release cache was not found"));
    }

    #[test]
    fn bytes_are_human_readable() {
        assert_eq!(format_bytes(999), "999 B");
        assert_eq!(format_bytes(1024), "1.0 KiB");
        assert_eq!(format_bytes(1024 * 1024), "1.0 MiB");
    }

    fn write_release(codex_root: &Path, name: &str, bytes: usize) {
        let release = codex_root
            .join("packages")
            .join("standalone")
            .join("releases")
            .join(name);
        fs::create_dir_all(&release).unwrap();
        fs::write(release.join("payload"), vec![b'x'; bytes]).unwrap();
    }

    fn release_names(releases: &[super::ReleaseInfo]) -> Vec<&str> {
        releases
            .iter()
            .map(|release| release.directory_name.as_str())
            .collect()
    }

    fn has_error(report: &super::VerificationReport, needle: &str) -> bool {
        report.checks.iter().any(|check| {
            check.status == VerificationStatus::Error && check.message.contains(needle)
        })
    }

    fn has_warning(report: &super::VerificationReport, needle: &str) -> bool {
        report.checks.iter().any(|check| {
            check.status == VerificationStatus::Warning && check.message.contains(needle)
        })
    }
}

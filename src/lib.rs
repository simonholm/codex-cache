use std::cmp::Reverse;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result, anyhow, bail};

const PACKAGES: &[&str] = &["standalone", "app-server-daemon"];
const EXPECTED_PACKAGE_ROOT_ENTRIES: &[&str] = &["current", "releases", "install.lock"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheScan {
    pub codex_root: PathBuf,
    pub packages: Vec<PackageScan>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageScan {
    pub name: String,
    pub root: PathBuf,
    pub releases_dir: PathBuf,
    pub current_target: Option<PathBuf>,
    pub current_version: Option<String>,
    pub releases: Vec<ReleaseInfo>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseInfo {
    pub package: String,
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
    pub releases_dir: PathBuf,
    pub current_target: Option<PathBuf>,
    pub releases_to_remove: Vec<ReleaseInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupResult {
    pub deleted: Vec<ReleaseInfo>,
    pub failures: Vec<CleanupFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupFailure {
    pub release: ReleaseInfo,
    pub message: String,
}

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
        self.packages.iter().map(PackageScan::total_size).sum()
    }

    pub fn current_size(&self) -> u64 {
        self.packages.iter().map(PackageScan::current_size).sum()
    }

    pub fn reclaim_if_only_current_kept(&self) -> u64 {
        self.total_size().saturating_sub(self.current_size())
    }

    pub fn reclaim_if_current_and_previous_kept(&self) -> u64 {
        self.packages
            .iter()
            .map(PackageScan::reclaim_if_current_and_previous_kept)
            .sum()
    }
}

impl PackageScan {
    pub fn total_size(&self) -> u64 {
        self.releases.iter().map(|release| release.size).sum()
    }

    pub fn current_size(&self) -> u64 {
        self.current_release().map_or(0, |release| release.size)
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

impl CleanupResult {
    pub fn reclaimed_bytes(&self) -> u64 {
        self.deleted.iter().map(|release| release.size).sum()
    }

    pub fn failed_count(&self) -> usize {
        self.failures.len()
    }

    pub fn success_count(&self) -> usize {
        self.deleted.len()
    }

    pub fn has_failures(&self) -> bool {
        !self.failures.is_empty()
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
    Ok(scan_default()?
        .packages
        .into_iter()
        .flat_map(|package| package.releases)
        .collect())
}

pub fn scan_releases_at(codex_root: &Path) -> Result<Vec<ReleaseInfo>> {
    Ok(scan_at(codex_root)?
        .packages
        .into_iter()
        .flat_map(|package| package.releases)
        .collect())
}

pub fn scan_at(codex_root: &Path) -> Result<CacheScan> {
    if !codex_root.exists() {
        bail!("Codex root was not found at {}", codex_root.display());
    }

    let mut packages = Vec::new();
    for name in PACKAGES {
        let root = codex_root.join("packages").join(name);
        if fs::symlink_metadata(&root).is_err() {
            continue;
        }
        packages.push(scan_package(name, &root)?);
    }
    if packages.is_empty() {
        bail!(
            "Codex release caches were not found under {}",
            codex_root.join("packages").display()
        );
    }

    Ok(CacheScan {
        codex_root: codex_root.to_path_buf(),
        packages,
    })
}

fn scan_package(name: &str, root: &Path) -> Result<PackageScan> {
    let releases_dir = root.join("releases");

    if !releases_dir.is_dir() {
        bail!(
            "Codex {name} release cache was not found at {}",
            releases_dir.display()
        );
    }

    let current_target = read_current_target(root)?;
    let current_name = current_target
        .as_deref()
        .and_then(|target| target.file_name())
        .map(|name| name.to_string_lossy().into_owned());
    let current_version = current_name
        .as_deref()
        .and_then(|name| parse_version(name).ok());

    let (releases, warnings) = discover_releases(name, &releases_dir, current_target.as_deref())?;

    Ok(PackageScan {
        name: name.to_string(),
        root: root.to_path_buf(),
        releases_dir,
        current_target,
        current_version,
        releases,
        warnings,
    })
}

pub fn render_scan(scan: &CacheScan) -> String {
    let mut output = String::new();
    output.push_str("Codex release cache scan\n");
    output.push_str(&format!("Codex root:      {}\n", scan.codex_root.display()));
    for package in &scan.packages {
        output.push_str(&format!("\n{}:\n", package.name));
        output.push_str(&format!("Package root:    {}\n", package.root.display()));
        output.push_str(&format!(
            "Releases dir:    {}\n",
            package.releases_dir.display()
        ));
        output.push_str(&format!(
            "Current target:  {}\n",
            package
                .current_target
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "not found".to_string())
        ));
        output.push_str(&format!(
            "Current version: {}\n",
            package.current_version.as_deref().unwrap_or("unknown")
        ));
        output.push_str("Installed releases:\n");
        if package.releases.is_empty() {
            output.push_str("  none\n");
        }
        for release in &package.releases {
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
        append_warnings(&mut output, &package.warnings);
    }
    output
}

pub fn render_report(scan: &CacheScan) -> String {
    let mut output = String::new();
    output.push_str("Codex release cache report\n");
    for package in &scan.packages {
        output.push_str(&format!("\n{}:\n", package.name));
        output.push_str(&format!(
            "Current version: {}\n",
            package.current_version.as_deref().unwrap_or("unknown")
        ));
        output.push_str(&format!(
            "Installed versions: {}\n",
            installed_versions(package)
        ));
    }
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
    let mut largest = scan
        .packages
        .iter()
        .flat_map(|package| &package.releases)
        .collect::<Vec<_>>();
    largest.sort_by_key(|release| Reverse(release.size));
    for release in largest.into_iter().take(5) {
        output.push_str(&format!(
            "  {:<17} {:<12} {:>10}  {}\n",
            release.package,
            release.display_version,
            format_bytes(release.size),
            release.path.display()
        ));
    }
    if has_no_releases(scan) {
        output.push_str("  none\n");
    }
    for package in &scan.packages {
        append_warnings(&mut output, &package.warnings);
    }
    output
}

pub fn render_list(scan: &CacheScan) -> String {
    let mut output = String::new();
    output.push_str("Package            Version     Active  Size\n");
    for release in scan.packages.iter().flat_map(|package| &package.releases) {
        output.push_str(&format!(
            "{:<18} {:<11} {:<7} {}\n",
            release.package,
            release.display_version,
            if release.active { "*" } else { "" },
            format_bytes(release.size)
        ));
    }
    if has_no_releases(scan) {
        output.push_str("none\n");
    }
    for package in &scan.packages {
        append_warnings(&mut output, &package.warnings);
    }
    output
}

fn has_no_releases(scan: &CacheScan) -> bool {
    scan.packages
        .iter()
        .all(|package| package.releases.is_empty())
}

pub fn plan_deletions(releases: &[ReleaseInfo], keep_policy: KeepPolicy) -> Result<DeletionPlan> {
    let releases_dir = releases
        .iter()
        .filter_map(|release| release.path.parent())
        .find(|path| !path.as_os_str().is_empty())
        .map_or_else(PathBuf::new, Path::to_path_buf);
    plan_deletions_in(&releases_dir, releases, keep_policy)
}

pub fn plan_deletions_in(
    releases_dir: &Path,
    releases: &[ReleaseInfo],
    keep_policy: KeepPolicy,
) -> Result<DeletionPlan> {
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
        releases_dir: releases_dir.to_path_buf(),
        current_target: Some(active.path.clone()),
        releases_to_remove,
    })
}

pub fn plan_scan_deletions(scan: &CacheScan, keep_policy: KeepPolicy) -> Result<Vec<DeletionPlan>> {
    scan.packages
        .iter()
        .map(|package| {
            plan_deletions_in(&package.releases_dir, &package.releases, keep_policy)
                .with_context(|| format!("failed to plan {} cleanup", package.name))
        })
        .collect()
}

pub fn clean_default(keep_policy: KeepPolicy) -> Result<CleanupResult> {
    let codex_root = codex_root()?;
    clean_at(&codex_root, keep_policy)
}

pub fn clean_at(codex_root: &Path, keep_policy: KeepPolicy) -> Result<CleanupResult> {
    let verification = verify_at(codex_root);
    if verification.error_count() > 0 {
        bail!("verification failed; clean aborted");
    }

    let scan = scan_at(codex_root)?;
    let plans = plan_scan_deletions(&scan, keep_policy)?;
    let mut result = CleanupResult {
        deleted: Vec::new(),
        failures: Vec::new(),
    };
    for plan in plans {
        let package_result = execute_deletion_plan(&plan);
        result.deleted.extend(package_result.deleted);
        result.failures.extend(package_result.failures);
    }
    Ok(result)
}

pub fn execute_deletion_plan(plan: &DeletionPlan) -> CleanupResult {
    execute_deletion_plan_with(plan, |path| fs::remove_dir_all(path))
}

fn execute_deletion_plan_with<F>(plan: &DeletionPlan, mut remove_dir_all: F) -> CleanupResult
where
    F: FnMut(&Path) -> std::io::Result<()>,
{
    let mut deleted = Vec::new();
    let mut failures = Vec::new();

    for release in &plan.releases_to_remove {
        if let Err(error) = validate_deletion_candidate(plan, release) {
            failures.push(CleanupFailure {
                release: release.clone(),
                message: error.to_string(),
            });
            continue;
        }

        match remove_dir_all(&release.path) {
            Ok(()) => deleted.push(release.clone()),
            Err(error) => failures.push(CleanupFailure {
                release: release.clone(),
                message: error.to_string(),
            }),
        }
    }

    CleanupResult { deleted, failures }
}

fn validate_deletion_candidate(plan: &DeletionPlan, release: &ReleaseInfo) -> Result<()> {
    if release.active {
        bail!("refusing to delete active release");
    }

    if plan
        .current_target
        .as_deref()
        .is_some_and(|target| same_path(target, &release.path))
    {
        bail!("refusing to delete current symlink target");
    }

    let package_root = plan
        .releases_dir
        .parent()
        .ok_or_else(|| anyhow!("invalid releases directory"))?;
    let current_target = read_current_target(package_root)?
        .ok_or_else(|| anyhow!("current symlink is missing; refusing deletion"))?;
    if same_path(&current_target, &release.path) {
        bail!("refusing to delete current symlink target");
    }

    if !path_is_inside(&release.path, &plan.releases_dir) {
        bail!(
            "refusing to delete path outside releases/: {}",
            release.path.display()
        );
    }

    if same_path(&release.path, &plan.releases_dir) {
        bail!("refusing to delete releases/ directory itself");
    }

    Ok(())
}

pub fn render_clean_dry_run(plan: &DeletionPlan) -> String {
    render_clean_dry_runs(std::slice::from_ref(plan))
}

pub fn render_clean_dry_runs(plans: &[DeletionPlan]) -> String {
    let mut output = String::new();
    output.push_str("Codex release cache clean dry run\n");
    if let Some(plan) = plans.first() {
        output.push_str(&format!("Keep policy: {}\n", plan.keep_policy.label()));
    }
    output.push_str("Would remove:\n");
    if plans.iter().all(|plan| plan.releases_to_remove.is_empty()) {
        output.push_str("  none\n");
    } else {
        for plan in plans {
            for release in &plan.releases_to_remove {
                output.push_str(&format!(
                    "  {}  {}\n",
                    release.package,
                    release.path.display()
                ));
            }
        }
    }
    let reclaimed = plans.iter().map(DeletionPlan::reclaimed_bytes).sum::<u64>();
    output.push_str(&format!("Reclaimed bytes: {}\n", reclaimed));
    output.push_str(&format!("Reclaimed size: {}\n", format_bytes(reclaimed)));
    output.push_str("Dry run: no files were deleted.\n");
    output
}

pub fn render_clean_result(result: &CleanupResult) -> String {
    let mut output = String::new();
    output.push_str("Codex release cache clean\n");
    output.push_str("Deleted:\n");
    if result.deleted.is_empty() {
        output.push_str("  none\n");
    } else {
        for release in &result.deleted {
            output.push_str(&format!(
                "  {}  {}\n",
                release.package,
                release.path.display()
            ));
        }
    }

    if !result.failures.is_empty() {
        output.push_str("Failed:\n");
        for failure in &result.failures {
            output.push_str(&format!(
                "  {}  {} ({})\n",
                failure.release.package,
                failure.release.path.display(),
                failure.message
            ));
        }
    }

    output.push_str(&format!("Reclaimed bytes: {}\n", result.reclaimed_bytes()));
    output.push_str(&format!(
        "Reclaimed size: {}\n",
        format_bytes(result.reclaimed_bytes())
    ));
    output.push_str(&format!(
        "Successful deletions: {}\n",
        result.success_count()
    ));
    output.push_str(&format!("Failed deletions: {}\n", result.failed_count()));
    output
}

pub fn verify_default() -> Result<VerificationReport> {
    let codex_root = codex_root()?;
    Ok(verify_at(&codex_root))
}

pub fn verify_at(codex_root: &Path) -> VerificationReport {
    let mut checks = Vec::new();
    let mut found = false;
    for name in PACKAGES {
        let root = codex_root.join("packages").join(name);
        if fs::symlink_metadata(&root).is_err() {
            continue;
        }
        found = true;
        let start = checks.len();
        verify_package(&mut checks, &root);
        for check in &mut checks[start..] {
            check.message = format!("{name}: {}", check.message);
        }
    }
    if !found {
        error_check(
            &mut checks,
            format!(
                "no Codex release cache found under {}",
                codex_root.join("packages").display()
            ),
        );
    }
    VerificationReport { checks }
}

fn verify_package(checks: &mut Vec<VerificationCheck>, root: &Path) {
    let releases_dir = root.join("releases");
    let current = root.join("current");

    check(
        checks,
        releases_dir.is_dir(),
        format!("releases directory exists: {}", releases_dir.display()),
        VerificationStatus::Error,
        format!("releases directory is missing: {}", releases_dir.display()),
    );

    let releases_readable = match fs::read_dir(&releases_dir) {
        Ok(_) => {
            pass(
                checks,
                format!("releases directory is readable: {}", releases_dir.display()),
            );
            true
        }
        Err(error) => {
            error_check(
                checks,
                format!(
                    "releases directory is not readable: {} ({error})",
                    releases_dir.display()
                ),
            );
            false
        }
    };

    warn_about_unexpected_layout(checks, root);

    let current_target = match fs::symlink_metadata(&current) {
        Ok(metadata) => {
            pass(
                checks,
                format!("current symlink exists: {}", current.display()),
            );

            if metadata.file_type().is_symlink() {
                pass(checks, "current is a symlink".to_string());
                match read_current_target(root) {
                    Ok(Some(target)) => {
                        if target.exists() {
                            pass(
                                checks,
                                format!("current target exists: {}", target.display()),
                            );
                        } else {
                            error_check(
                                checks,
                                format!("current symlink is dangling: {}", target.display()),
                            );
                        }

                        check(
                            checks,
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
                        error_check(checks, format!("failed to read current symlink: {error}"));
                        None
                    }
                }
            } else {
                error_check(checks, "current exists but is not a symlink".to_string());
                None
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            error_check(
                checks,
                format!("current symlink is missing: {}", current.display()),
            );
            None
        }
        Err(error) => {
            error_check(
                checks,
                format!(
                    "failed to inspect current symlink: {} ({error})",
                    current.display()
                ),
            );
            None
        }
    };

    if releases_readable {
        match discover_releases("", &releases_dir, current_target.as_deref()) {
            Ok((releases, warnings)) => {
                for warning in warnings {
                    warning_check(checks, warning);
                }

                if current_target.is_some() {
                    let active_count = releases.iter().filter(|release| release.active).count();
                    check(
                        checks,
                        active_count == 1,
                        "active release appears exactly once".to_string(),
                        VerificationStatus::Error,
                        format!("active release appears {active_count} times"),
                    );
                } else {
                    error_check(
                        checks,
                        "active release cannot be checked without a readable current symlink"
                            .to_string(),
                    );
                }
            }
            Err(error) => error_check(
                checks,
                format!("failed to inspect release directories: {error}"),
            ),
        }
    }
}

pub fn render_verify(report: &VerificationReport) -> String {
    let mut output = String::new();
    output.push_str("Codex release cache verification\n");
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
    package: &str,
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
            package: package.to_string(),
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

fn warn_about_unexpected_layout(checks: &mut Vec<VerificationCheck>, package_root: &Path) {
    let entries = match fs::read_dir(package_root) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !EXPECTED_PACKAGE_ROOT_ENTRIES.contains(&name.as_str()) {
            warning_check(
                checks,
                format!(
                    "unexpected entry under package root: {}",
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

fn installed_versions(scan: &PackageScan) -> String {
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
        KeepPolicy, VerificationStatus, clean_at, execute_deletion_plan,
        execute_deletion_plan_with, format_bytes, plan_deletions, plan_deletions_in,
        plan_scan_deletions, render_clean_dry_run, render_clean_dry_runs, render_clean_result,
        render_list, render_report, render_scan, render_verify, scan_at, verify_at,
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

        assert_eq!(scan.packages[0].current_version.as_deref(), Some("0.145.0"));
        assert_eq!(scan.packages[0].releases.len(), 2);
        assert_eq!(scan.packages[0].releases[0].display_version, "0.145.0");
        assert!(scan.packages[0].releases[0].active);
        assert_eq!(scan.packages[0].releases[1].display_version, "0.144.5");
        assert!(!scan.packages[0].releases[1].active);
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

        assert_eq!(scan.packages[0].releases.len(), 2);
        assert_eq!(
            scan.packages[0].current_release().unwrap().directory_name,
            "0.145.0-x86_64-unknown-linux-musl"
        );
        assert_eq!(
            scan.packages[0]
                .releases
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

        assert!(output.starts_with("Package            Version     Active  Size\n"));
        assert!(output.contains("standalone         0.145.0     *       30 B\n"));
        assert!(output.contains("standalone         0.144.5             20 B\n"));
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

        assert_eq!(scan.packages[0].releases.len(), 1);
        assert_eq!(scan.packages[0].warnings.len(), 1);
        assert!(scan.packages[0].warnings[0].contains("latest"));
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
        let plan = plan_deletions(&scan.packages[0].releases, KeepPolicy::Current).unwrap();

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
        let plan = plan_deletions(&scan.packages[0].releases, KeepPolicy::CurrentPrevious).unwrap();

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
        let error = plan_deletions(&scan.packages[0].releases, KeepPolicy::Current)
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
        let plan = plan_deletions(&scan.packages[0].releases, KeepPolicy::Current).unwrap();
        let output = render_clean_dry_run(&plan);

        assert!(output.contains("Keep policy: current\n"));
        assert!(output.contains("0.144.5-x86_64-unknown-linux-musl"));
        assert!(output.contains("Reclaimed bytes: 2048\n"));
        assert!(output.contains("Reclaimed size: 2.0 KiB\n"));
        assert!(output.contains("Dry run: no files were deleted.\n"));
    }

    #[test]
    fn clean_deletes_selected_release_directories() {
        let directory = tempfile::tempdir().unwrap();
        write_release(directory.path(), "0.144.5-x86_64-unknown-linux-musl", 20);
        write_release(directory.path(), "0.145.0-x86_64-unknown-linux-musl", 30);
        fs::write(
            directory
                .path()
                .join("packages")
                .join("standalone")
                .join("install.lock"),
            b"",
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

        let result = clean_at(directory.path(), KeepPolicy::Current).unwrap();
        let output = render_clean_result(&result);

        assert_eq!(result.success_count(), 1);
        assert_eq!(result.failed_count(), 0);
        assert_eq!(result.reclaimed_bytes(), 20);
        assert!(output.contains("Codex release cache clean\n"));
        assert!(output.contains("Reclaimed bytes: 20\n"));
        assert!(output.contains("Successful deletions: 1\n"));
        assert!(output.contains("Failed deletions: 0\n"));
        assert!(
            !directory
                .path()
                .join("packages")
                .join("standalone")
                .join("releases")
                .join("0.144.5-x86_64-unknown-linux-musl")
                .exists()
        );
        assert!(
            directory
                .path()
                .join("packages")
                .join("standalone")
                .join("releases")
                .join("0.145.0-x86_64-unknown-linux-musl")
                .exists()
        );
    }

    #[test]
    fn clean_reports_partial_deletion_failure_and_continues() {
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
        let plan = plan_deletions_in(
            &scan.packages[0].releases_dir,
            &scan.packages[0].releases,
            KeepPolicy::Current,
        )
        .unwrap();
        let failing_release = "0.144.5-x86_64-unknown-linux-musl";
        let result = execute_deletion_plan_with(&plan, |path| {
            if path.file_name().unwrap() == failing_release {
                Err(std::io::Error::other("simulated delete failure"))
            } else {
                fs::remove_dir_all(path)
            }
        });
        let output = render_clean_result(&result);

        assert_eq!(result.success_count(), 1);
        assert_eq!(result.failed_count(), 1);
        assert_eq!(result.reclaimed_bytes(), 10);
        assert!(output.contains("Failed:\n"));
        assert!(output.contains("simulated delete failure"));
        assert!(output.contains("Successful deletions: 1\n"));
        assert!(output.contains("Failed deletions: 1\n"));
        assert!(
            !directory
                .path()
                .join("packages")
                .join("standalone")
                .join("releases")
                .join("0.144.0-x86_64-unknown-linux-musl")
                .exists()
        );
        assert!(
            directory
                .path()
                .join("packages")
                .join("standalone")
                .join("releases")
                .join("0.144.5-x86_64-unknown-linux-musl")
                .exists()
        );
    }

    #[test]
    fn clean_aborts_when_current_symlink_is_invalid() {
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

        let error = clean_at(directory.path(), KeepPolicy::Current)
            .unwrap_err()
            .to_string();

        assert_eq!(error, "verification failed; clean aborted");
        assert!(
            directory
                .path()
                .join("packages")
                .join("standalone")
                .join("releases")
                .join("0.145.0-x86_64-unknown-linux-musl")
                .exists()
        );
    }

    #[test]
    fn execute_deletion_plan_refuses_paths_outside_releases() {
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
        let outside = directory.path().join("outside-release");
        fs::create_dir_all(&outside).unwrap();
        let mut plan = plan_deletions_in(
            &scan.packages[0].releases_dir,
            &scan.packages[0].releases,
            KeepPolicy::Current,
        )
        .unwrap();
        plan.releases_to_remove[0].path = outside.clone();

        let result = execute_deletion_plan(&plan);

        assert_eq!(result.success_count(), 0);
        assert_eq!(result.failed_count(), 1);
        assert!(result.failures[0].message.contains("outside releases/"));
        assert!(outside.exists());
    }

    #[test]
    fn execute_deletion_plan_protects_current_after_symlink_changes() {
        let directory = tempfile::tempdir().unwrap();
        write_release(directory.path(), "0.144.0", 10);
        write_release(directory.path(), "0.145.0", 20);
        link_current(directory.path(), "standalone", "0.145.0");
        let scan = scan_at(directory.path()).unwrap();
        let plan = plan_deletions_in(
            &scan.packages[0].releases_dir,
            &scan.packages[0].releases,
            KeepPolicy::Current,
        )
        .unwrap();
        let current = directory.path().join("packages/standalone/current");
        fs::remove_file(&current).unwrap();
        symlink("releases/0.144.0", &current).unwrap();

        let result = execute_deletion_plan(&plan);
        assert_eq!(result.success_count(), 0);
        assert_eq!(result.failed_count(), 1);
        assert!(
            result.failures[0]
                .message
                .contains("current symlink target")
        );
        assert!(
            directory
                .path()
                .join("packages/standalone/releases/0.144.0")
                .exists()
        );
    }

    #[test]
    fn verify_valid_installation_reports_success_summary() {
        let directory = tempfile::tempdir().unwrap();
        write_release(directory.path(), "0.145.0-x86_64-unknown-linux-musl", 30);
        fs::write(
            directory
                .path()
                .join("packages")
                .join("standalone")
                .join("install.lock"),
            b"",
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
        let output = render_verify(&report);

        assert_eq!(report.error_count(), 0);
        assert_eq!(report.warning_count(), 0);
        assert!(output.contains("PASS    standalone: active release appears exactly once"));
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
    fn verify_unknown_file_under_standalone_root_reports_warning_only() {
        let directory = tempfile::tempdir().unwrap();
        write_release(directory.path(), "0.145.0-x86_64-unknown-linux-musl", 30);
        fs::write(
            directory
                .path()
                .join("packages")
                .join("standalone")
                .join("notes.txt"),
            b"",
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

        assert!(has_warning(&report, "unexpected entry under package root"));
        assert!(has_warning(&report, "notes.txt"));
        assert_eq!(report.warning_count(), 1);
        assert_eq!(report.error_count(), 0);
    }

    #[test]
    fn verify_unknown_directory_under_standalone_root_reports_warning_only() {
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

        assert!(has_warning(&report, "unexpected entry under package root"));
        assert!(has_warning(&report, "scratch"));
        assert_eq!(report.warning_count(), 1);
        assert_eq!(report.error_count(), 0);
    }

    #[test]
    fn verify_mixed_expected_and_unexpected_standalone_entries_only_warns_for_unexpected() {
        let directory = tempfile::tempdir().unwrap();
        write_release(directory.path(), "0.145.0-x86_64-unknown-linux-musl", 30);
        fs::write(
            directory
                .path()
                .join("packages")
                .join("standalone")
                .join("install.lock"),
            b"",
        )
        .unwrap();
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

        assert!(has_warning(&report, "scratch"));
        assert!(!has_warning(&report, "install.lock"));
        assert_eq!(report.warning_count(), 1);
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

        assert!(error.contains("Codex release caches were not found"));
    }

    #[test]
    fn two_packages_have_independent_current_and_previous_releases() {
        let directory = tempfile::tempdir().unwrap();
        for (package, versions, current) in [
            ("standalone", ["0.155.0", "0.156.0", "0.157.0"], "0.157.0"),
            (
                "app-server-daemon",
                ["0.153.0", "0.154.0", "0.155.0"],
                "0.154.0",
            ),
        ] {
            for version in versions {
                write_package_release(directory.path(), package, version, 10);
            }
            link_current(directory.path(), package, current);
        }

        let scan = scan_at(directory.path()).unwrap();
        assert_eq!(scan.packages.len(), 2);
        assert_eq!(scan.packages[0].current_version.as_deref(), Some("0.157.0"));
        assert_eq!(scan.packages[1].current_version.as_deref(), Some("0.154.0"));
        assert_eq!(scan.reclaim_if_only_current_kept(), 40);
        assert_eq!(scan.reclaim_if_current_and_previous_kept(), 20);
        assert!(render_report(&scan).contains("app-server-daemon:\nCurrent version: 0.154.0"));
        assert!(render_list(&scan).contains("app-server-daemon  0.154.0"));

        let plans = plan_scan_deletions(&scan, KeepPolicy::CurrentPrevious).unwrap();
        assert_eq!(plans.len(), 2);
        assert_eq!(release_names(&plans[0].releases_to_remove), vec!["0.155.0"]);
        assert_eq!(release_names(&plans[1].releases_to_remove), vec!["0.153.0"]);
        let dry_run = render_clean_dry_runs(&plans);
        assert!(dry_run.contains("standalone  "));
        assert!(dry_run.contains("app-server-daemon  "));
        assert_eq!(verify_at(directory.path()).error_count(), 0);
    }

    #[test]
    fn clean_both_packages_preserves_each_current_target() {
        let directory = tempfile::tempdir().unwrap();
        for (package, current) in [("standalone", "0.157.0"), ("app-server-daemon", "0.156.0")] {
            write_package_release(directory.path(), package, "0.155.0", 10);
            write_package_release(directory.path(), package, current, 20);
            link_current(directory.path(), package, current);
        }

        let result = clean_at(directory.path(), KeepPolicy::Current).unwrap();
        assert_eq!(result.success_count(), 2);
        for (package, current) in [("standalone", "0.157.0"), ("app-server-daemon", "0.156.0")] {
            let root = directory.path().join("packages").join(package);
            assert!(root.join("current").is_symlink());
            assert!(root.join("releases").join(current).exists());
            assert!(!root.join("releases/0.155.0").exists());
        }
    }

    #[test]
    fn missing_package_is_skipped_but_invalid_present_package_blocks_clean() {
        let directory = tempfile::tempdir().unwrap();
        write_package_release(directory.path(), "app-server-daemon", "0.157.0", 20);
        write_package_release(directory.path(), "app-server-daemon", "0.156.0", 10);
        link_current(directory.path(), "app-server-daemon", "0.157.0");
        let scan = scan_at(directory.path()).unwrap();
        assert_eq!(scan.packages.len(), 1);
        assert_eq!(scan.packages[0].name, "app-server-daemon");
        assert_eq!(verify_at(directory.path()).error_count(), 0);
        assert_eq!(
            clean_at(directory.path(), KeepPolicy::Current)
                .unwrap()
                .success_count(),
            1
        );

        fs::create_dir_all(directory.path().join("packages/standalone")).unwrap();
        let report = verify_at(directory.path());
        assert!(has_error(
            &report,
            "standalone: releases directory is missing"
        ));
        assert!(clean_at(directory.path(), KeepPolicy::Current).is_err());
    }

    #[test]
    fn verify_reports_each_package_error_without_hiding_the_other() {
        let directory = tempfile::tempdir().unwrap();
        write_package_release(directory.path(), "standalone", "0.157.0", 10);
        write_package_release(directory.path(), "app-server-daemon", "0.156.0", 10);
        link_current(directory.path(), "standalone", "0.157.0");
        link_current(directory.path(), "app-server-daemon", "0.999.0");

        let report = verify_at(directory.path());
        let output = render_verify(&report);
        assert!(output.contains("standalone: active release appears exactly once"));
        assert!(has_error(
            &report,
            "app-server-daemon: current symlink is dangling"
        ));
        assert!(clean_at(directory.path(), KeepPolicy::Current).is_err());
        assert!(
            directory
                .path()
                .join("packages/standalone/releases/0.157.0")
                .exists()
        );
    }

    #[test]
    fn bytes_are_human_readable() {
        assert_eq!(format_bytes(999), "999 B");
        assert_eq!(format_bytes(1024), "1.0 KiB");
        assert_eq!(format_bytes(1024 * 1024), "1.0 MiB");
    }

    fn write_release(codex_root: &Path, name: &str, bytes: usize) {
        write_package_release(codex_root, "standalone", name, bytes);
    }

    fn write_package_release(codex_root: &Path, package: &str, name: &str, bytes: usize) {
        let release = codex_root
            .join("packages")
            .join(package)
            .join("releases")
            .join(name);
        fs::create_dir_all(&release).unwrap();
        fs::write(release.join("payload"), vec![b'x'; bytes]).unwrap();
    }

    fn link_current(codex_root: &Path, package: &str, name: &str) {
        symlink(
            format!("releases/{name}"),
            codex_root.join("packages").join(package).join("current"),
        )
        .unwrap();
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

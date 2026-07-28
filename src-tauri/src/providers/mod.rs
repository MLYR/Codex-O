use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};

const SKILL_MARKDOWN_FILE: &str = "SKILL.md";
const PLUGIN_MANIFEST_DIRECTORY: &str = ".codex-plugin";
const PLUGIN_MANIFEST_FILE: &str = "plugin.json";
const PLUGIN_SKILLS_DIRECTORY: &str = "skills";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    UserGlobal,
    Repo,
    LegacyUser,
    System,
    Plugin,
    Bundled,
    AdditionalRoot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct ProviderCapabilities {
    pub can_read: bool,
    pub can_import: bool,
    pub can_quarantine: bool,
    pub can_restore: bool,
    pub can_update: bool,
    pub can_delete: bool,
}

impl ProviderCapabilities {
    const fn read_only() -> Self {
        Self {
            can_read: true,
            can_import: false,
            can_quarantine: false,
            can_restore: false,
            can_update: false,
            can_delete: false,
        }
    }

    const fn managed_user() -> Self {
        Self {
            can_read: true,
            can_import: true,
            can_quarantine: true,
            can_restore: true,
            can_update: false,
            can_delete: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ProviderDescriptor {
    pub id: String,
    pub kind: ProviderKind,
    pub display_name: String,
    pub capabilities: ProviderCapabilities,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DiscoveredSkill {
    pub id: String,
    pub provider_id: String,
    pub provider_kind: ProviderKind,
    pub relative_path: String,
    #[serde(skip_serializing)]
    skill_directory: PathBuf,
}

impl DiscoveredSkill {
    pub(crate) fn skill_directory(&self) -> &Path {
        &self.skill_directory
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryWarningCode {
    EntryUnreadable,
    InvalidRelativePath,
    InvalidSkillMarker,
    RootUnavailable,
    SymlinkDenied,
    UnsupportedCacheLayout,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DiscoveryWarning {
    pub code: DiscoveryWarningCode,
    pub provider_id: String,
    pub relative_path: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderDiagnosticCode {
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ProviderDiagnostic {
    pub provider_id: String,
    pub kind: ProviderKind,
    pub code: ProviderDiagnosticCode,
}

#[derive(Default, Debug, PartialEq, Eq, Serialize)]
pub struct ProviderDiscovery {
    pub providers: Vec<ProviderDescriptor>,
    pub skills: Vec<DiscoveredSkill>,
    pub warnings: Vec<DiscoveryWarning>,
    pub diagnostics: Vec<ProviderDiagnostic>,
}

impl ProviderDiscovery {
    fn extend(&mut self, mut other: Self) {
        self.providers.append(&mut other.providers);
        self.skills.append(&mut other.skills);
        self.warnings.append(&mut other.warnings);
        self.diagnostics.append(&mut other.diagnostics);
    }

    fn warn(
        &mut self,
        code: DiscoveryWarningCode,
        descriptor: &ProviderDescriptor,
        relative_path: Option<String>,
    ) {
        self.warnings.push(DiscoveryWarning {
            code,
            provider_id: descriptor.id.clone(),
            relative_path,
        });
    }
}

pub trait SkillProvider {
    fn descriptor(&self) -> ProviderDescriptor;
    fn discover(&self) -> ProviderDiscovery;
}

#[derive(Clone, Debug)]
pub struct ProviderRoots {
    pub home_directory: PathBuf,
    pub repository_directory: PathBuf,
    pub plugin_cache_directory: PathBuf,
    pub additional_roots: Vec<AdditionalRoot>,
    pub include_plugin_cache: bool,
}

impl ProviderRoots {
    pub fn new(
        home_directory: PathBuf,
        repository_directory: PathBuf,
        plugin_cache_directory: PathBuf,
    ) -> Self {
        Self {
            home_directory,
            repository_directory,
            plugin_cache_directory,
            additional_roots: Vec::new(),
            include_plugin_cache: true,
        }
    }

    pub fn with_additional_roots(mut self, additional_roots: Vec<AdditionalRoot>) -> Self {
        self.additional_roots = additional_roots;
        self
    }

    pub fn with_plugin_cache_enabled(mut self, enabled: bool) -> Self {
        self.include_plugin_cache = enabled;
        self
    }
}

#[derive(Clone, Debug)]
pub struct AdditionalRoot {
    id: String,
    directory: PathBuf,
}

impl AdditionalRoot {
    pub fn new(id: impl Into<String>, directory: PathBuf) -> Option<Self> {
        let id = id.into();

        if id.is_empty()
            || id.len() > 64
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        {
            return None;
        }

        Some(Self { id, directory })
    }
}

pub struct ProviderRegistry {
    user_global: DirectoryProvider,
    repo: DirectoryProvider,
    legacy_user: DirectoryProvider,
    plugin_cache: PluginCacheProvider,
    plugin_cache_enabled: bool,
    additional_roots: Vec<DirectoryProvider>,
}

impl ProviderRegistry {
    pub fn with_roots(roots: ProviderRoots) -> Self {
        let additional_roots = roots
            .additional_roots
            .into_iter()
            .map(|root| {
                DirectoryProvider::new(
                    descriptor(
                        &format!("additional_root:{}", root.id),
                        ProviderKind::AdditionalRoot,
                    ),
                    root.directory,
                )
            })
            .collect();

        Self {
            user_global: DirectoryProvider::new(
                descriptor("user_global", ProviderKind::UserGlobal),
                roots.home_directory.join(".agents/skills"),
            ),
            repo: DirectoryProvider::new(
                descriptor("repo", ProviderKind::Repo),
                roots.repository_directory.join(".agents/skills"),
            ),
            legacy_user: DirectoryProvider::new(
                descriptor("legacy_user", ProviderKind::LegacyUser),
                roots.home_directory.join(".codex/skills"),
            ),
            plugin_cache: PluginCacheProvider::new(roots.plugin_cache_directory),
            plugin_cache_enabled: roots.include_plugin_cache,
            additional_roots,
        }
    }

    pub fn discover_all(&self) -> ProviderDiscovery {
        let mut discovery = ProviderDiscovery::default();

        discovery.extend(self.user_global.discover());
        discovery.extend(self.repo.discover());
        discovery.extend(self.legacy_user.discover());
        if self.plugin_cache_enabled {
            discovery.extend(self.plugin_cache.discover());
        }
        for additional_root in &self.additional_roots {
            discovery.extend(additional_root.discover());
        }
        discovery
            .providers
            .push(descriptor("system", ProviderKind::System));
        discovery.diagnostics.push(ProviderDiagnostic {
            provider_id: "system".to_owned(),
            kind: ProviderKind::System,
            code: ProviderDiagnosticCode::Unavailable,
        });

        discovery
    }
}

struct DirectoryProvider {
    descriptor: ProviderDescriptor,
    root: PathBuf,
}

impl DirectoryProvider {
    fn new(descriptor: ProviderDescriptor, root: PathBuf) -> Self {
        Self { descriptor, root }
    }
}

impl SkillProvider for DirectoryProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        self.descriptor.clone()
    }

    fn discover(&self) -> ProviderDiscovery {
        let mut discovery = discover_skill_directories(&self.descriptor, &self.root);
        discovery.providers.push(self.descriptor());
        if !is_regular_directory(&self.root) {
            discovery.diagnostics.push(ProviderDiagnostic {
                provider_id: self.descriptor.id.clone(),
                kind: self.descriptor.kind,
                code: ProviderDiagnosticCode::Unavailable,
            });
        }
        discovery
    }
}

struct PluginCacheProvider {
    root: PathBuf,
}

impl PluginCacheProvider {
    fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

impl SkillProvider for PluginCacheProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        descriptor("plugin_cache", ProviderKind::Plugin)
    }

    fn discover(&self) -> ProviderDiscovery {
        let cache_descriptor = self.descriptor();
        let mut discovery = ProviderDiscovery::default();
        let Some(channels) =
            child_directories(&self.root, &self.root, &cache_descriptor, &mut discovery)
        else {
            return discovery;
        };

        for (channel_path, channel_name) in channels {
            let Some(plugins) =
                child_directories(&channel_path, &self.root, &cache_descriptor, &mut discovery)
            else {
                continue;
            };

            for (plugin_path, plugin_name) in plugins {
                let Some(versions) =
                    child_directories(&plugin_path, &self.root, &cache_descriptor, &mut discovery)
                else {
                    continue;
                };

                for (version_path, version_name) in versions {
                    let kind = if channel_name == "openai-bundled" {
                        ProviderKind::Bundled
                    } else {
                        ProviderKind::Plugin
                    };
                    let descriptor = ProviderDescriptor {
                        id: format!(
                            "{}:{channel_name}:{plugin_name}:{version_name}",
                            provider_kind_id(kind)
                        ),
                        kind,
                        display_name: plugin_name.clone(),
                        capabilities: ProviderCapabilities::read_only(),
                    };
                    let manifest_directory = version_path.join(PLUGIN_MANIFEST_DIRECTORY);
                    let manifest_path = manifest_directory.join(PLUGIN_MANIFEST_FILE);
                    let skills_root = version_path.join(PLUGIN_SKILLS_DIRECTORY);

                    if is_symlink(&manifest_directory)
                        || is_symlink(&manifest_path)
                        || is_symlink(&skills_root)
                    {
                        discovery.warn(
                            DiscoveryWarningCode::SymlinkDenied,
                            &descriptor,
                            relative_path(&self.root, &version_path),
                        );
                        continue;
                    }

                    if !is_regular_directory(&manifest_directory)
                        || !is_regular_file(&manifest_path)
                        || !is_regular_directory(&skills_root)
                    {
                        discovery.warn(
                            DiscoveryWarningCode::UnsupportedCacheLayout,
                            &descriptor,
                            relative_path(&self.root, &version_path),
                        );
                        continue;
                    }

                    discovery.providers.push(descriptor.clone());
                    discovery.extend(discover_skill_directories(&descriptor, &skills_root));
                }
            }
        }

        discovery
    }
}

fn descriptor(id: &str, kind: ProviderKind) -> ProviderDescriptor {
    let (display_name, capabilities) = match kind {
        ProviderKind::UserGlobal => ("User Global", ProviderCapabilities::managed_user()),
        ProviderKind::Repo => ("Repository", ProviderCapabilities::read_only()),
        ProviderKind::LegacyUser => ("Legacy User", ProviderCapabilities::read_only()),
        ProviderKind::System => ("System", ProviderCapabilities::read_only()),
        ProviderKind::Plugin => ("Plugin", ProviderCapabilities::read_only()),
        ProviderKind::Bundled => ("Bundled", ProviderCapabilities::read_only()),
        ProviderKind::AdditionalRoot => ("Additional Root", ProviderCapabilities::read_only()),
    };

    ProviderDescriptor {
        id: id.to_owned(),
        kind,
        display_name: display_name.to_owned(),
        capabilities,
    }
}

fn provider_kind_id(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::UserGlobal => "user_global",
        ProviderKind::Repo => "repo",
        ProviderKind::LegacyUser => "legacy_user",
        ProviderKind::System => "system",
        ProviderKind::Plugin => "plugin",
        ProviderKind::Bundled => "bundled",
        ProviderKind::AdditionalRoot => "additional_root",
    }
}

fn discover_skill_directories(descriptor: &ProviderDescriptor, root: &Path) -> ProviderDiscovery {
    let mut discovery = ProviderDiscovery::default();
    scan_skill_directories(descriptor, root, root, &mut discovery);
    discovery
}

fn scan_skill_directories(
    descriptor: &ProviderDescriptor,
    root: &Path,
    directory: &Path,
    discovery: &mut ProviderDiscovery,
) {
    let Some(entries) = child_directories(directory, root, descriptor, discovery) else {
        return;
    };

    for (entry_path, relative) in entries {
        let marker_path = entry_path.join(SKILL_MARKDOWN_FILE);
        match fs::symlink_metadata(&marker_path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                discovery.warn(
                    DiscoveryWarningCode::SymlinkDenied,
                    descriptor,
                    Some(format!("{relative}/{SKILL_MARKDOWN_FILE}")),
                );
                scan_skill_directories(descriptor, root, &entry_path, discovery);
            }
            Ok(metadata) if metadata.is_file() => {
                discovery.skills.push(DiscoveredSkill {
                    id: skill_id(&descriptor.id, &relative),
                    provider_id: descriptor.id.clone(),
                    provider_kind: descriptor.kind,
                    relative_path: relative,
                    skill_directory: entry_path,
                });
            }
            Ok(_) => {
                discovery.warn(
                    DiscoveryWarningCode::InvalidSkillMarker,
                    descriptor,
                    Some(format!("{relative}/{SKILL_MARKDOWN_FILE}")),
                );
                scan_skill_directories(descriptor, root, &entry_path, discovery);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                scan_skill_directories(descriptor, root, &entry_path, discovery);
            }
            Err(_) => {
                discovery.warn(
                    DiscoveryWarningCode::EntryUnreadable,
                    descriptor,
                    Some(format!("{relative}/{SKILL_MARKDOWN_FILE}")),
                );
            }
        }
    }
}

fn skill_id(provider_id: &str, relative_path: &str) -> String {
    format!("skill:{}:{provider_id}{relative_path}", provider_id.len())
}

fn child_directories(
    directory: &Path,
    root: &Path,
    descriptor: &ProviderDescriptor,
    discovery: &mut ProviderDiscovery,
) -> Option<Vec<(PathBuf, String)>> {
    match fs::symlink_metadata(directory) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            discovery.warn(
                DiscoveryWarningCode::SymlinkDenied,
                descriptor,
                relative_path(root, directory),
            );
            return None;
        }
        Ok(metadata) if !metadata.is_dir() => {
            discovery.warn(
                DiscoveryWarningCode::EntryUnreadable,
                descriptor,
                relative_path(root, directory),
            );
            return None;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Some(Vec::new()),
        Err(_) => {
            discovery.warn(
                DiscoveryWarningCode::RootUnavailable,
                descriptor,
                relative_path(root, directory),
            );
            return None;
        }
        Ok(_) => {}
    }

    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(_) => {
            discovery.warn(
                DiscoveryWarningCode::RootUnavailable,
                descriptor,
                relative_path(root, directory),
            );
            return None;
        }
    };
    let mut directories = Vec::new();

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                discovery.warn(DiscoveryWarningCode::EntryUnreadable, descriptor, None);
                continue;
            }
        };
        let path = entry.path();
        let relative = match relative_path(root, &path) {
            Some(relative) => relative,
            None => {
                discovery.warn(DiscoveryWarningCode::InvalidRelativePath, descriptor, None);
                continue;
            }
        };
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => {
                discovery.warn(
                    DiscoveryWarningCode::EntryUnreadable,
                    descriptor,
                    Some(relative),
                );
                continue;
            }
        };

        if metadata.file_type().is_symlink() {
            discovery.warn(
                DiscoveryWarningCode::SymlinkDenied,
                descriptor,
                Some(relative),
            );
        } else if metadata.is_dir() {
            directories.push((path, relative));
        }
    }

    directories.sort_by(|left, right| left.1.cmp(&right.1));
    Some(directories)
}

fn is_regular_file(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
        .unwrap_or(false)
}

fn is_regular_directory(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
        .unwrap_or(false)
}

fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
}

fn relative_path(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let mut components = Vec::new();

    for component in relative.components() {
        match component {
            Component::Normal(value) => components.push(value.to_str()?.to_owned()),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }

    Some(components.join("/"))
}

#[cfg(test)]
mod tests;

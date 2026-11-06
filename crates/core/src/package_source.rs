// SPDX-License-Identifier: MIT OR Apache-2.0
//! Where package sources may come from, and optional repository metadata.

use reqwest::Url;
use std::path::{Component, Path, PathBuf};

/// What kind of source is being checked, for diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Index,
    Package,
    Git,
}

impl SourceKind {
    fn label(self) -> &'static str {
        match self {
            Self::Index => "package index",
            Self::Package => "package source",
            Self::Git => "Git source",
        }
    }
}

/// Hosts, URLs and local paths a package source must match.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceAllowlist {
    rules: Vec<Rule>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Rule {
    Host {
        host: String,
        port: Option<u16>,
        suffix: bool,
    },
    Url {
        scheme: String,
        host: String,
        port: Option<u16>,
        path: String,
        prefix: bool,
    },
    Path {
        path: PathBuf,
        prefix: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SourceRef {
    Url {
        scheme: String,
        host: String,
        port: Option<u16>,
        path: String,
    },
    Path(PathBuf),
}

impl SourceAllowlist {
    /// Parse `forge.opt.package_allowed_sources`. An empty list denies every
    /// source; callers use `None` outside for "not configured".
    pub fn new(entries: &[String]) -> Result<Self, String> {
        let mut rules = Vec::with_capacity(entries.len());
        for entry in entries {
            rules.push(parse_rule(entry)?);
        }
        Ok(Self { rules })
    }

    /// Is `source` allowed by at least one rule?
    pub fn allows(&self, source: &str) -> bool {
        let Ok(source) = parse_source(source) else {
            return false;
        };
        self.rules.iter().any(|rule| rule.matches(&source))
    }

    /// A user-facing denial, with credentials removed.
    pub fn check(&self, kind: SourceKind, source: &str) -> Result<(), String> {
        if self.allows(source) {
            Ok(())
        } else {
            Err(format!(
                "{} {} is not allowed by forge.opt.package_allowed_sources",
                kind.label(),
                display_source(source)
            ))
        }
    }

    /// Check a redirect target before following it.
    pub fn check_redirect(&self, source: &str) -> Result<(), String> {
        if self.allows(source) {
            Ok(())
        } else {
            Err(format!(
                "redirect to {} is not allowed by forge.opt.package_allowed_sources",
                display_source(source)
            ))
        }
    }
}

impl Rule {
    fn matches(&self, source: &SourceRef) -> bool {
        match (self, source) {
            (
                Self::Host { host, port, suffix },
                SourceRef::Url {
                    scheme: source_scheme,
                    host: source_host,
                    port: source_port,
                    ..
                },
            ) => {
                let host_matches = if *suffix {
                    source_host.len() > host.len()
                        && source_host.ends_with(host)
                        && source_host.as_bytes()[source_host.len() - host.len() - 1] == b'.'
                } else {
                    source_host == host
                };
                host_matches
                    && port.is_none_or(|port| {
                        Some(port) == effective_port(source_scheme, *source_port)
                    })
            }
            (
                Self::Url {
                    scheme,
                    host,
                    port,
                    path,
                    prefix,
                },
                SourceRef::Url {
                    scheme: source_scheme,
                    host: source_host,
                    port: source_port,
                    path: source_path,
                },
            ) => {
                scheme == source_scheme
                    && host == source_host
                    && effective_port(scheme, *port) == effective_port(source_scheme, *source_port)
                    && if *prefix {
                        source_path.starts_with(path)
                    } else {
                        source_path == path
                            || source_path
                                .strip_suffix(".minisig")
                                .is_some_and(|p| p == path)
                    }
            }
            (Self::Path { path, prefix }, SourceRef::Path(source)) => {
                if *prefix {
                    source.starts_with(path)
                } else {
                    source == path
                }
            }
            _ => false,
        }
    }
}

fn parse_rule(entry: &str) -> Result<Rule, String> {
    let entry = entry.trim();
    if entry.is_empty() {
        return Err(invalid(entry, "empty source rule"));
    }
    if entry.contains("://") {
        if entry.contains('*') {
            return Err(invalid(entry, "URL rules do not support wildcards"));
        }
        let url = Url::parse(entry).map_err(|e| invalid(entry, &e.to_string()))?;
        if url.scheme() == "file" {
            return path_rule(entry);
        }
        let Some(host) = url.host_str() else {
            return Err(invalid(entry, "a URL rule needs a host"));
        };
        let scheme = url.scheme().to_ascii_lowercase();
        if !matches!(scheme.as_str(), "http" | "https" | "ssh" | "git") {
            return Err(invalid(
                entry,
                "supported URL schemes are http, https, ssh, git and file",
            ));
        }
        let prefix = url.path().ends_with('/');
        return Ok(Rule::Url {
            scheme,
            host: host.to_ascii_lowercase(),
            port: url.port(),
            path: url.path().to_string(),
            prefix,
        });
    }
    if let Some((host, path)) = scp_parts(entry)? {
        return Ok(Rule::Url {
            scheme: "ssh".into(),
            host: host.to_ascii_lowercase(),
            port: None,
            path,
            prefix: false,
        });
    }
    if let Some(host) = entry.strip_prefix("*.") {
        if host.is_empty() || host.contains(':') || host.split('.').any(str::is_empty) {
            return Err(invalid(entry, "bad host suffix"));
        }
        return Ok(Rule::Host {
            host: host.to_ascii_lowercase(),
            port: None,
            suffix: true,
        });
    }
    if entry.contains('*') {
        return Err(invalid(
            entry,
            "only a leading *. host wildcard is supported",
        ));
    }
    if entry.starts_with("~/") || Path::new(entry).is_absolute() {
        return path_rule(entry);
    }
    if entry.contains(['/', '?', '#', '@']) {
        return Err(invalid(
            entry,
            "use a host, a URL, an absolute path, or ~/path",
        ));
    }
    let url =
        Url::parse(&format!("forge://{entry}/")).map_err(|_| invalid(entry, "bad host or port"))?;
    let Some(host) = url.host_str() else {
        return Err(invalid(entry, "bad host or port"));
    };
    if !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid(entry, "bad host or port"));
    }
    Ok(Rule::Host {
        host: host.to_ascii_lowercase(),
        port: url.port(),
        suffix: false,
    })
}

fn path_rule(entry: &str) -> Result<Rule, String> {
    let raw = if entry.starts_with("file://") {
        let url = Url::parse(entry).map_err(|e| invalid(entry, &e.to_string()))?;
        url.to_file_path()
            .map_err(|_| invalid(entry, "a file URL must name a local path"))?
    } else {
        crate::permissions::expand_home(entry)
    };
    if !raw.is_absolute() {
        return Err(invalid(
            entry,
            "a path rule must be absolute or start with ~/",
        ));
    }
    if raw.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(invalid(entry, "a path rule may not contain .."));
    }
    let prefix = entry.ends_with('/') || entry.ends_with(std::path::MAIN_SEPARATOR);
    Ok(Rule::Path {
        path: normalize_path(&raw).ok_or_else(|| invalid(entry, "bad path"))?,
        prefix,
    })
}

fn parse_source(source: &str) -> Result<SourceRef, ()> {
    if source.contains("://") {
        let url = Url::parse(source).map_err(|_| ())?;
        if url.scheme() == "file" {
            return url
                .to_file_path()
                .ok()
                .and_then(|p| normalize_path(&p))
                .map(SourceRef::Path)
                .ok_or(());
        }
        let host = url.host_str().ok_or(())?;
        return Ok(SourceRef::Url {
            scheme: url.scheme().to_ascii_lowercase(),
            host: host.to_ascii_lowercase(),
            port: url.port(),
            path: url.path().to_string(),
        });
    }
    if let Some((host, path)) = scp_parts(source).map_err(|_| ())? {
        return Ok(SourceRef::Url {
            scheme: "ssh".into(),
            host: host.to_ascii_lowercase(),
            port: None,
            path,
        });
    }
    let path = crate::permissions::expand_home(source);
    if !path.is_absolute() {
        return Err(());
    }
    normalize_path(&path).map(SourceRef::Path).ok_or(())
}

fn scp_parts(source: &str) -> Result<Option<(String, String)>, String> {
    let Some(at) = source.find('@') else {
        return Ok(None);
    };
    let rest = &source[at + 1..];
    let Some(colon) = rest.find(':') else {
        return Ok(None);
    };
    if rest.find('/').is_some_and(|slash| slash < colon) {
        return Ok(None);
    }
    let host = &rest[..colon];
    let path = &rest[colon + 1..];
    if host.is_empty() || path.is_empty() {
        return Err("bad scp-style Git source".into());
    }
    Ok(Some((
        host.to_string(),
        if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{path}")
        },
    )))
}

fn normalize_path(path: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    return None;
                }
            }
            Component::Normal(part) => out.push(part),
        }
    }
    out.is_absolute().then_some(out)
}

fn effective_port(scheme: &str, port: Option<u16>) -> Option<u16> {
    port.or(match scheme {
        "http" => Some(80),
        "https" => Some(443),
        "ssh" => Some(22),
        "git" => Some(9418),
        _ => None,
    })
}

fn display_source(source: &str) -> String {
    let Ok(mut url) = Url::parse(source) else {
        return source.to_string();
    };
    if !url.username().is_empty() || url.password().is_some() {
        let _ = url.set_username("");
        let _ = url.set_password(None);
    }
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
}

fn invalid(entry: &str, reason: &str) -> String {
    format!("forge.opt.package_allowed_sources: invalid source rule {entry:?}: {reason}")
}

/// Provider metadata used only for advisory warnings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepositoryMetadata {
    pub archived: Option<bool>,
    /// The provider's canonical `owner/name`, when it reports one.
    pub canonical: Option<String>,
}

/// Optional repository API lookup. Tests replace this with a fixture.
pub trait RepositoryInfo: Send + Sync {
    fn inspect(&self, repository: &str) -> Result<RepositoryMetadata, String>;
}

/// GitHub's public repository metadata API, with a short timeout.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemRepositoryInfo;

impl RepositoryInfo for SystemRepositoryInfo {
    fn inspect(&self, repository: &str) -> Result<RepositoryMetadata, String> {
        let Some(slug) = github_slug(repository) else {
            return Ok(RepositoryMetadata::default());
        };
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| e.to_string())?;
        let response = client
            .get(format!("https://api.github.com/repos/{slug}"))
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .header(
                reqwest::header::USER_AGENT,
                concat!("forge/", env!("CARGO_PKG_VERSION")),
            )
            .send()
            .map_err(|e| e.to_string())?;
        if !response.status().is_success() {
            return Err(format!(
                "{repository}: repository API answered {}",
                response.status()
            ));
        }
        let body = response.text().map_err(|e| e.to_string())?;
        let value: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
        Ok(RepositoryMetadata {
            archived: value.get("archived").and_then(serde_json::Value::as_bool),
            canonical: value
                .get("full_name")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
        })
    }
}

/// Warnings for an archived or renamed repository. They never block.
pub fn repository_warnings(source: &str, metadata: &RepositoryMetadata) -> Vec<String> {
    let mut warnings = Vec::new();
    let slug = github_slug(source);
    if metadata.archived == Some(true) {
        match &slug {
            Some(slug) => warnings.push(format!(
                "WARNING: GitHub repository {slug} is archived; it will not receive upstream updates."
            )),
            None => warnings.push(
                "WARNING: the upstream repository is archived; it will not receive updates."
                    .to_string(),
            ),
        }
    }
    if let (Some(current), Some(canonical)) = (slug, metadata.canonical.as_deref()) {
        let canonical = canonical.trim().trim_end_matches(".git");
        if !canonical.is_empty() && !canonical.eq_ignore_ascii_case(&current) {
            warnings.push(format!(
                "WARNING: GitHub repository {current} now identifies itself as {canonical}; the owner or name changed."
            ));
        }
    }
    warnings
}

fn github_slug(source: &str) -> Option<String> {
    let (host, path) = if source.contains("://") {
        let url = Url::parse(source).ok()?;
        (url.host_str()?.to_ascii_lowercase(), url.path().to_string())
    } else {
        let (host, path) = scp_parts(source).ok()??;
        (host.to_ascii_lowercase(), path)
    };
    if host != "github.com" {
        return None;
    }
    let mut parts = path.trim_matches('/').split('/');
    let owner = parts.next().filter(|s| !s.is_empty())?;
    let name = parts.next().filter(|s| !s.is_empty())?;
    if parts.next().is_some() {
        return None;
    }
    Some(format!("{owner}/{}", name.trim_end_matches(".git")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allow(entries: &[&str]) -> SourceAllowlist {
        SourceAllowlist::new(&entries.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap()
    }

    #[test]
    fn exact_hosts_match_any_transport_and_port() {
        let list = allow(&["github.com"]);
        assert!(list.allows("https://github.com/a/b"));
        assert!(list.allows("git@github.com:a/b.git"));
        assert!(list.allows("ssh://git@github.com:2222/a/b"));
        assert!(allow(&["github.com:443"]).allows("https://github.com/a/b"));
        assert!(!allow(&["github.com:443"]).allows("https://github.com:8443/a/b"));
        assert!(!list.allows("https://api.github.com/a/b"));
        assert!(!list.allows("https://github.com.evil/a/b"));
    }

    #[test]
    fn host_suffixes_require_a_dot_boundary_and_skip_the_apex() {
        let list = allow(&["*.example.org"]);
        assert!(list.allows("https://api.example.org/x"));
        assert!(list.allows("https://a.b.example.org/x"));
        assert!(!list.allows("https://example.org/x"));
        assert!(!list.allows("https://badexample.org/x"));
    }

    #[test]
    fn exact_urls_match_one_source_and_cover_their_signature() {
        let list = allow(&["https://e.org/forge/index.toml"]);
        assert!(list.allows("https://e.org/forge/index.toml"));
        assert!(list.allows("https://e.org/forge/index.toml.minisig"));
        assert!(!list.allows("https://e.org/forge/index.toml.evil"));
        assert!(!list.allows("http://e.org/forge/index.toml"));
        assert!(!list.allows("https://e.org:8443/forge/index.toml"));
    }

    #[test]
    fn directory_prefixes_stop_at_a_path_boundary() {
        let list = allow(&["https://e.org/forge/"]);
        assert!(list.allows("https://e.org/forge/index.toml"));
        assert!(list.allows("https://e.org/forge/a/b.wasm"));
        assert!(!list.allows("https://e.org/forge-other/a.wasm"));
        assert!(!list.allows("https://e.org/Forge/a.wasm"));
    }

    #[test]
    fn local_paths_are_absolute_and_prefixes_are_component_aware() {
        let root = std::env::temp_dir();
        let prefix = format!("{}/forge-pkgs/", root.display());
        let exact = format!("{}/forge-pkgs/index.toml", root.display());
        let list = allow(&[&prefix, &exact]);
        assert!(list.allows(&exact));
        let child = format!("{}/forge-pkgs/a.wasm", root.display());
        assert!(list.allows(&child));
        assert!(list.allows(&format!("file://{child}")));
        let sibling = format!("{}/forge-pkgs-other/a.wasm", root.display());
        assert!(!list.allows(&sibling));
        assert!(!list.allows(&format!("{}/forge-pkgs/../secret", root.display())));
    }

    #[test]
    fn exact_local_file_does_not_allow_siblings() {
        let root = std::env::temp_dir();
        let file = format!("{}/forge-one.wasm", root.display());
        let list = allow(&[&file]);
        assert!(list.allows(&file));
        assert!(!list.allows(&format!("{}/forge-two.wasm", root.display())));
    }

    #[test]
    fn scp_git_rules_match_ssh_urls_and_ignore_userinfo() {
        let list = allow(&["ssh://git@github.com/acme/"]);
        assert!(list.allows("git@github.com:acme/plugin.git"));
        assert!(list.allows("ssh://git@github.com/acme/plugin.git"));
        assert!(!list.allows("https://github.com/acme/plugin"));
    }

    #[test]
    fn an_empty_allowlist_denies_everything() {
        let list = allow(&[]);
        assert!(!list.allows("https://e.org/x"));
        assert!(!list.allows("/tmp/x"));
    }

    #[test]
    fn credentials_are_redacted_in_errors() {
        let list = allow(&["e.org"]);
        let error = list
            .check(
                SourceKind::Package,
                "https://user:secret@evil.example/x?token=hidden",
            )
            .unwrap_err();
        assert!(!error.contains("secret"), "{error}");
        assert!(!error.contains("token=hidden"), "{error}");
        assert!(error.contains("evil.example"), "{error}");
    }

    #[test]
    fn redirects_are_checked_without_leaking_credentials() {
        let list = allow(&["e.org"]);
        assert!(list.check_redirect("https://e.org/a.wasm").is_ok());
        let error = list
            .check_redirect("https://user:secret@evil.test/a.wasm?token=abc")
            .unwrap_err();
        assert!(error.contains("evil.test"), "{error}");
        assert!(!error.contains("secret"), "{error}");
        assert!(!error.contains("token"), "{error}");
    }

    #[test]
    fn invalid_rules_are_rejected() {
        for entry in [
            "",
            "relative/path",
            "*.",
            "https://",
            "/a/../b",
            "https://e.org/*",
        ] {
            assert!(
                SourceAllowlist::new(&[entry.to_string()]).is_err(),
                "{entry:?}"
            );
        }
    }

    #[test]
    fn repository_warnings_name_archived_and_renamed_repositories() {
        let metadata = RepositoryMetadata {
            archived: Some(true),
            canonical: Some("new/name".into()),
        };
        let warnings = repository_warnings("https://github.com/old/repo", &metadata);
        assert_eq!(warnings.len(), 2);
        assert!(warnings[0].contains("old/repo") && warnings[0].contains("archived"));
        assert!(warnings[1].contains("new/name") && warnings[1].contains("owner or name changed"));
    }
}

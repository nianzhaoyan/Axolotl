//! Functions for fetching information from the Internet
use super::io::{self, IOError};
use crate::event::LoadingBarId;
use crate::event::emit::emit_loading;
use crate::{ErrorKind, LabrinthError};
use bytes::Bytes;
use chrono::{DateTime, TimeDelta, Utc};
use eyre::{Context, eyre};
use futures::StreamExt;
use parking_lot::Mutex;
use rand::Rng;
use reqwest::{Method, StatusCode, header};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256, Sha512};
use std::collections::{HashMap, VecDeque};
use std::ffi::OsStr;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Once, Weak};
use std::time::{self, Instant};
use tokio::sync::{Mutex as AsyncMutex, Semaphore};
use tokio::{
    fs::{File, OpenOptions},
    io::{AsyncReadExt, AsyncWriteExt},
};
use url::Url;

pub const DOWNLOAD_META_HEADER: &str = "modrinth-download-meta";

const BMCLAPI_BASE_URL: &str = "https://bmclapi2.bangbang93.com";
const MCIM_BASE_URL: &str = "https://mod.mcimirror.top";
const TENCENT_MAVEN_BASE_URL: &str =
    "https://mirrors.cloud.tencent.com/nexus/repository/maven-public";
const ARTIFACT_ATTEMPT_BUDGET: usize = 4;
const SEGMENTED_DOWNLOAD_THRESHOLD: u64 = 32 * 1024 * 1024;
const MIN_SEGMENT_SIZE: u64 = 8 * 1024 * 1024;
const MAX_REDIRECT_LOCATION_BYTES: usize = 8 * 1024;

#[derive(
    Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ResourceClass {
    Metadata,
    MinecraftAsset,
    MinecraftLibrary,
    Loader,
    Java,
    Modrinth,
    CurseForge,
    Modpack,
    #[default]
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadRouteSource {
    Official,
    Bmclapi,
    Mcim,
    TencentMaven,
    Alternate,
}

impl DownloadRouteSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Official => "official",
            Self::Bmclapi => "bmclapi",
            Self::Mcim => "mcim",
            Self::TencentMaven => "tencent_maven",
            Self::Alternate => "alternate",
        }
    }
}

#[derive(
    Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ProxyPolicy {
    #[default]
    System,
    Direct,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DownloadRoute {
    pub url: String,
    pub source: DownloadRouteSource,
    pub is_mirror: bool,
    pub allow_sensitive_headers: bool,
    pub supports_range: bool,
    pub proxy: ProxyPolicy,
}

#[derive(
    Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ContentValidation {
    #[default]
    None,
    Json,
    Jar,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Integrity {
    pub size: Option<u64>,
    pub sha1: Option<String>,
    pub sha512: Option<String>,
    pub sha256: Option<String>,
    pub md5: Option<String>,
    pub content: ContentValidation,
}

impl Integrity {
    pub fn sha1(hash: impl Into<String>) -> Self {
        Self {
            sha1: Some(hash.into()),
            ..Self::default()
        }
    }

    pub fn sha512(hash: impl Into<String>) -> Self {
        Self {
            sha512: Some(hash.into()),
            ..Self::default()
        }
    }

    pub fn sha256(hash: impl Into<String>) -> Self {
        Self {
            sha256: Some(hash.into()),
            ..Self::default()
        }
    }

    pub fn md5(hash: impl Into<String>) -> Self {
        Self {
            md5: Some(hash.into()),
            ..Self::default()
        }
    }

    pub fn with_sha1(mut self, hash: impl Into<String>) -> Self {
        self.sha1 = Some(hash.into());
        self
    }

    pub fn with_sha512(mut self, hash: impl Into<String>) -> Self {
        self.sha512 = Some(hash.into());
        self
    }

    pub fn with_sha256(mut self, hash: impl Into<String>) -> Self {
        self.sha256 = Some(hash.into());
        self
    }

    pub fn with_md5(mut self, hash: impl Into<String>) -> Self {
        self.md5 = Some(hash.into());
        self
    }

    fn has_verified_content_hash(&self) -> bool {
        self.sha1.is_some() || self.sha256.is_some() || self.sha512.is_some()
    }

    pub fn with_size(mut self, size: u64) -> Self {
        self.size = Some(size);
        self
    }

    pub fn with_content_validation(
        mut self,
        content: ContentValidation,
    ) -> Self {
        self.content = content;
        self
    }

    fn is_empty(&self) -> bool {
        self.size.is_none()
            && self.sha1.is_none()
            && self.sha512.is_none()
            && self.sha256.is_none()
            && self.md5.is_none()
            && self.content == ContentValidation::None
    }
}

#[derive(
    Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ResumePolicy {
    Disabled,
    #[default]
    Safe,
}

#[derive(Clone, Debug)]
pub struct DownloadRequest {
    pub url: String,
    pub resource: ResourceClass,
    pub source_mode: Option<crate::state::DownloadSourceMode>,
    pub integrity: Integrity,
    pub resume: ResumePolicy,
    pub download_meta: Option<DownloadMeta>,
    pub header: Option<(String, String)>,
    pub routes: Option<Vec<DownloadRoute>>,
    pub candidate_urls: Vec<String>,
}

impl DownloadRequest {
    pub fn new(url: impl Into<String>, resource: ResourceClass) -> Self {
        Self {
            url: url.into(),
            resource,
            source_mode: None,
            integrity: Integrity::default(),
            resume: ResumePolicy::Safe,
            download_meta: None,
            header: None,
            routes: None,
            candidate_urls: Vec::new(),
        }
    }

    pub fn with_integrity(mut self, integrity: Integrity) -> Self {
        self.integrity = integrity;
        self
    }

    pub fn with_resume_policy(mut self, resume: ResumePolicy) -> Self {
        self.resume = resume;
        self
    }

    pub fn with_download_meta(mut self, download_meta: DownloadMeta) -> Self {
        self.download_meta = Some(download_meta);
        self
    }

    pub fn with_header(
        mut self,
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        self.header = Some((name.into(), value.into()));
        self
    }

    pub fn with_source_mode(
        mut self,
        source_mode: crate::state::DownloadSourceMode,
    ) -> Self {
        self.source_mode = Some(source_mode);
        self
    }

    pub fn with_routes(mut self, routes: Vec<DownloadRoute>) -> Self {
        self.routes = Some(routes);
        self
    }

    pub fn with_candidate_urls<I, S>(mut self, urls: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.candidate_urls.extend(urls.into_iter().map(Into::into));
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DownloadResult {
    pub path: PathBuf,
    pub url: String,
    pub source: DownloadRouteSource,
    pub size: u64,
    pub resumed_bytes: u64,
    pub attempts: usize,
    pub fallback_count: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct ResumeMetadata {
    url: String,
    etag: Option<String>,
    last_modified: Option<String>,
    expected_size: Option<u64>,
    sha1: Option<String>,
    sha512: Option<String>,
    sha256: Option<String>,
    md5: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct HostHealth {
    successes: u64,
    failures: u64,
    consecutive_failures: u32,
    ttfb_ewma_ms: Option<f64>,
    throughput_ewma_bps: Option<f64>,
    cooldown_until: Option<Instant>,
    range_disabled_until: Option<Instant>,
}

impl HostHealth {
    fn score(&self) -> f64 {
        let failure_rate = self.failures as f64
            / (self.successes + self.failures).max(1) as f64;
        failure_rate * 10_000.0 + self.ttfb_ewma_ms.unwrap_or(1_500.0)
            - self.throughput_ewma_bps.unwrap_or(0.0) / 1_000_000.0
    }

    fn is_cooling_down(&self) -> bool {
        self.cooldown_until
            .is_some_and(|until| until > Instant::now())
    }
}

static HOST_HEALTH: LazyLock<Mutex<HashMap<String, HostHealth>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static HOST_HEALTH_LOADED: Once = Once::new();
static HOST_HEALTH_PERSIST_SCHEDULED: AtomicBool = AtomicBool::new(false);
static HOST_HEALTH_DIRTY: AtomicBool = AtomicBool::new(false);
static IN_FLIGHT_DOWNLOADS: LazyLock<
    dashmap::DashMap<String, Weak<AsyncMutex<()>>>,
> = LazyLock::new(dashmap::DashMap::new);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DownloadSourceHealth {
    pub host: String,
    pub successes: u64,
    pub failures: u64,
    pub ttfb_ms: Option<f64>,
    pub throughput_bytes_per_second: Option<f64>,
    pub cooling_down: bool,
    pub range_splitting_disabled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PersistedHostHealth {
    host: String,
    successes: u64,
    failures: u64,
    consecutive_failures: u32,
    ttfb_ewma_ms: Option<f64>,
    throughput_ewma_bps: Option<f64>,
    cooldown_seconds_remaining: Option<u64>,
    #[serde(default)]
    range_disabled_seconds_remaining: Option<u64>,
}

fn host_health_path() -> Option<PathBuf> {
    crate::State::get_if_initialized().map(|state| {
        state
            .directories
            .caches_dir()
            .join("download-source-health.json")
    })
}

fn ensure_host_health_loaded() {
    if host_health_path().is_none() {
        return;
    }
    HOST_HEALTH_LOADED.call_once(|| {
        let path =
            host_health_path().expect("state was checked before loading");
        let Ok(bytes) = std::fs::read(&path) else {
            return;
        };
        let Ok(entries) =
            serde_json::from_slice::<Vec<PersistedHostHealth>>(&bytes)
        else {
            tracing::warn!(
                path = %path.display(),
                "Ignoring invalid persisted download source health"
            );
            return;
        };
        let now = Instant::now();
        let mut health = HOST_HEALTH.lock();
        for entry in entries {
            health.insert(
                entry.host,
                HostHealth {
                    successes: entry.successes,
                    failures: entry.failures,
                    consecutive_failures: entry.consecutive_failures,
                    ttfb_ewma_ms: entry.ttfb_ewma_ms,
                    throughput_ewma_bps: entry.throughput_ewma_bps,
                    cooldown_until: entry.cooldown_seconds_remaining.map(
                        |seconds| {
                            now + time::Duration::from_secs(
                                seconds.min(15 * 60),
                            )
                        },
                    ),
                    range_disabled_until: entry
                        .range_disabled_seconds_remaining
                        .map(|seconds| {
                            now + time::Duration::from_secs(
                                seconds.min(24 * 60 * 60),
                            )
                        }),
                },
            );
        }
    });
}

fn persisted_host_health() -> Vec<PersistedHostHealth> {
    let now = Instant::now();
    HOST_HEALTH
        .lock()
        .iter()
        .map(|(host, health)| PersistedHostHealth {
            host: host.clone(),
            successes: health.successes,
            failures: health.failures,
            consecutive_failures: health.consecutive_failures,
            ttfb_ewma_ms: health.ttfb_ewma_ms,
            throughput_ewma_bps: health.throughput_ewma_bps,
            cooldown_seconds_remaining: health
                .cooldown_until
                .and_then(|until| until.checked_duration_since(now))
                .map(|duration| duration.as_secs()),
            range_disabled_seconds_remaining: health
                .range_disabled_until
                .and_then(|until| until.checked_duration_since(now))
                .map(|duration| duration.as_secs()),
        })
        .collect()
}

fn schedule_host_health_persist() {
    HOST_HEALTH_DIRTY.store(true, Ordering::Relaxed);
    if HOST_HEALTH_PERSIST_SCHEDULED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        HOST_HEALTH_PERSIST_SCHEDULED.store(false, Ordering::Release);
        return;
    };
    runtime.spawn(async {
        tokio::time::sleep(time::Duration::from_secs(2)).await;
        HOST_HEALTH_DIRTY.store(false, Ordering::Release);
        if let Some(path) = host_health_path() {
            let result = async {
                if let Some(parent) = path.parent() {
                    io::create_dir_all(parent).await?;
                }
                let bytes = serde_json::to_vec(&persisted_host_health())?;
                io::write(&path, bytes).await?;
                Ok::<_, crate::Error>(())
            }
            .await;
            if let Err(error) = result {
                tracing::warn!(
                    path = %path.display(),
                    error = %error,
                    "Unable to persist download source health"
                );
            }
        }
        HOST_HEALTH_PERSIST_SCHEDULED.store(false, Ordering::Release);
        if HOST_HEALTH_DIRTY.swap(false, Ordering::AcqRel) {
            schedule_host_health_persist();
        }
    });
}

pub fn download_source_health() -> Vec<DownloadSourceHealth> {
    ensure_host_health_loaded();
    let health = HOST_HEALTH.lock();
    let mut snapshots = health
        .iter()
        .map(|(host, health)| DownloadSourceHealth {
            host: host.clone(),
            successes: health.successes,
            failures: health.failures,
            ttfb_ms: health.ttfb_ewma_ms,
            throughput_bytes_per_second: health.throughput_ewma_bps,
            cooling_down: health.is_cooling_down(),
            range_splitting_disabled: health
                .range_disabled_until
                .is_some_and(|until| until > Instant::now()),
        })
        .collect::<Vec<_>>();
    snapshots.sort_by(|left, right| left.host.cmp(&right.host));
    snapshots
}

pub fn reset_download_source_health() {
    ensure_host_health_loaded();
    HOST_HEALTH.lock().clear();
    schedule_host_health_persist();
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct DownloadMirrorSettings {
    pub minecraft: bool,
    pub modrinth: bool,
    pub curseforge: bool,
}

impl DownloadMirrorSettings {
    pub fn current() -> Self {
        crate::State::get_if_initialized()
            .map(|state| Self {
                minecraft: state.use_minecraft_mirror(),
                modrinth: state.use_modrinth_mirror(),
                curseforge: state.use_curseforge_mirror(),
            })
            .unwrap_or_default()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedDownloadUrl {
    pub url: String,
    pub is_mirror: bool,
}

fn modrinth_request_kind(url: &str) -> Option<&'static str> {
    if url.starts_with(env!("MODRINTH_API_URL"))
        || url.starts_with(env!("MODRINTH_API_URL_V3"))
    {
        Some("API")
    } else if url.starts_with("https://cdn.modrinth.com") {
        Some("CDN")
    } else {
        None
    }
}

fn is_safe_redirect_location(location: &str) -> bool {
    location.len() <= MAX_REDIRECT_LOCATION_BYTES && location.is_ascii()
}

fn is_official_modrinth_cdn_redirect(location: Option<&str>) -> bool {
    let Some(location) = location.filter(|location| {
        is_safe_redirect_location(location)
            && location
                .get(..8)
                .is_some_and(|scheme| scheme.eq_ignore_ascii_case("https://"))
    }) else {
        return false;
    };
    let authority = location[8..]
        .split(|character| matches!(character, '/' | '?' | '#'))
        .next()
        .unwrap_or_default();
    authority.eq_ignore_ascii_case("cdn.modrinth.com")
        || authority.eq_ignore_ascii_case("cdn.modrinth.com:443")
}

fn is_mrpack_url(url: &str) -> bool {
    reqwest::Url::parse(url)
        .ok()
        .is_some_and(|url| url.path().to_ascii_lowercase().ends_with(".mrpack"))
}

fn route(
    url: String,
    source: DownloadRouteSource,
    is_mirror: bool,
    supports_range: bool,
) -> DownloadRoute {
    DownloadRoute {
        url,
        source,
        is_mirror,
        allow_sensitive_headers: !is_mirror,
        supports_range,
        proxy: ProxyPolicy::System,
    }
}

fn official_route(url: &str, resource: ResourceClass) -> DownloadRoute {
    let source = Url::parse(url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .map_or(DownloadRouteSource::Official, |host| match host.as_str() {
            "bmclapi2.bangbang93.com" => DownloadRouteSource::Bmclapi,
            "mod.mcimirror.top" => DownloadRouteSource::Mcim,
            "mirrors.cloud.tencent.com" => DownloadRouteSource::TencentMaven,
            _ => DownloadRouteSource::Official,
        });
    let is_mirror = matches!(
        source,
        DownloadRouteSource::Bmclapi
            | DownloadRouteSource::Mcim
            | DownloadRouteSource::TencentMaven
    );
    route(
        url.to_string(),
        source,
        is_mirror,
        !matches!(resource, ResourceClass::Metadata),
    )
}

fn url_with_base(original: &Url, base: &str, path: &str) -> Option<String> {
    let mut target = Url::parse(base).ok()?;
    target.set_path(path);
    target.set_query(original.query());
    target.set_fragment(None);
    Some(target.into())
}

fn explicit_mirror_routes(
    url: &str,
    resource: ResourceClass,
) -> Vec<DownloadRoute> {
    let Ok(parsed) = Url::parse(url) else {
        return Vec::new();
    };
    if parsed.scheme() != "https" {
        return Vec::new();
    }

    let host = parsed.host_str().unwrap_or_default();
    let path = parsed.path();
    let supports_range = !matches!(resource, ResourceClass::Metadata);
    let mut routes = Vec::new();
    let push_mirror = |routes: &mut Vec<DownloadRoute>,
                       base: &str,
                       path: String,
                       source: DownloadRouteSource| {
        if let Some(url) = url_with_base(&parsed, base, &path) {
            routes.push(route(url, source, true, supports_range));
        }
    };

    match host {
        "resources.download.minecraft.net" => {
            push_mirror(
                &mut routes,
                BMCLAPI_BASE_URL,
                format!("/assets{path}"),
                DownloadRouteSource::Bmclapi,
            );
        }
        "libraries.minecraft.net" => {
            push_mirror(
                &mut routes,
                BMCLAPI_BASE_URL,
                format!("/maven{path}"),
                DownloadRouteSource::Bmclapi,
            );
            push_mirror(
                &mut routes,
                BMCLAPI_BASE_URL,
                format!("/libraries{path}"),
                DownloadRouteSource::Bmclapi,
            );
        }
        "maven.minecraftforge.net" | "maven.fabricmc.net" => {
            push_mirror(
                &mut routes,
                BMCLAPI_BASE_URL,
                format!("/maven{path}"),
                DownloadRouteSource::Bmclapi,
            );
        }
        "files.minecraftforge.net" if path.starts_with("/maven/") => {
            push_mirror(
                &mut routes,
                BMCLAPI_BASE_URL,
                path.to_string(),
                DownloadRouteSource::Bmclapi,
            );
        }
        "maven.neoforged.net" if path.starts_with("/releases/") => {
            push_mirror(
                &mut routes,
                BMCLAPI_BASE_URL,
                format!("/maven/{}", path.trim_start_matches("/releases/")),
                DownloadRouteSource::Bmclapi,
            );
        }
        "repo1.maven.org" | "repo.maven.apache.org"
            if path.starts_with("/maven2/") =>
        {
            push_mirror(
                &mut routes,
                TENCENT_MAVEN_BASE_URL,
                format!(
                    "/nexus/repository/maven-public/{}",
                    path.trim_start_matches("/maven2/")
                ),
                DownloadRouteSource::TencentMaven,
            );
        }
        "meta.fabricmc.net" => {
            push_mirror(
                &mut routes,
                BMCLAPI_BASE_URL,
                format!("/fabric-meta{path}"),
                DownloadRouteSource::Bmclapi,
            );
        }
        "piston-meta.mojang.com"
        | "launchermeta.mojang.com"
        | "launcher.mojang.com"
        | "piston-data.mojang.com" => push_mirror(
            &mut routes,
            BMCLAPI_BASE_URL,
            path.to_string(),
            DownloadRouteSource::Bmclapi,
        ),
        "api.modrinth.com" => push_mirror(
            &mut routes,
            MCIM_BASE_URL,
            format!("/modrinth{path}"),
            DownloadRouteSource::Mcim,
        ),
        "cdn.modrinth.com" => push_mirror(
            &mut routes,
            MCIM_BASE_URL,
            path.to_string(),
            DownloadRouteSource::Mcim,
        ),
        "api.curseforge.com" => push_mirror(
            &mut routes,
            MCIM_BASE_URL,
            format!("/curseforge{path}"),
            DownloadRouteSource::Mcim,
        ),
        "edge.forgecdn.net"
        | "media.forgecdn.net"
        | "mediafilez.forgecdn.net" => push_mirror(
            &mut routes,
            MCIM_BASE_URL,
            path.to_string(),
            DownloadRouteSource::Mcim,
        ),
        _ => {}
    }

    routes
}

fn likely_mainland_china() -> bool {
    const LOCALE_KEYS: [&str; 4] = ["LC_ALL", "LC_MESSAGES", "LANG", "TZ"];
    let locale_match = LOCALE_KEYS.iter().any(|key| {
        std::env::var(key).is_ok_and(|value| {
            let value = value.to_ascii_lowercase();
            value.contains("zh_cn")
                || value.contains("zh-cn")
                || value.contains("asia/shanghai")
                || value.contains("asia/chongqing")
        })
    });
    if locale_match {
        return true;
    }

    #[cfg(unix)]
    {
        if let Ok(timezone) = std::fs::read_link("/etc/localtime") {
            let timezone = timezone.to_string_lossy().to_ascii_lowercase();
            if timezone.contains("asia/shanghai")
                || timezone.contains("asia/chongqing")
            {
                return true;
            }
        }
    }

    false
}

fn route_host(route: &DownloadRoute) -> Option<String> {
    Url::parse(&route.url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
}

fn is_official_modrinth_download_url(url: &str) -> bool {
    Url::parse(url).is_ok_and(|url| {
        matches!(
            url.host_str(),
            Some("api.modrinth.com" | "cdn.modrinth.com")
        )
    })
}

fn order_auto_routes(routes: &mut [DownloadRoute]) {
    ensure_host_health_loaded();
    let mainland = likely_mainland_china();
    let health = HOST_HEALTH.lock();
    routes.sort_by(|left, right| {
        let route_score = |route: &DownloadRoute| {
            let baseline = match (mainland, route.is_mirror) {
                (true, true) | (false, false) => 0.0,
                _ => 500.0,
            };
            let host_health = route_host(route)
                .and_then(|host| health.get(&host).cloned())
                .unwrap_or_default();
            let cooldown = if host_health.is_cooling_down() {
                1_000_000.0
            } else {
                0.0
            };
            baseline + host_health.score() + cooldown
        };
        route_score(left).total_cmp(&route_score(right))
    });
}

pub fn resolve_download_routes_for(
    url: &str,
    resource: ResourceClass,
    mode: crate::state::DownloadSourceMode,
) -> Vec<DownloadRoute> {
    let official = official_route(url, resource);
    if mode == crate::state::DownloadSourceMode::OfficialOnly {
        let mut routes = vec![official];
        if resource == ResourceClass::CurseForge && !routes[0].is_mirror {
            let mut direct = routes[0].clone();
            direct.proxy = ProxyPolicy::Direct;
            routes.push(direct);
        }
        return routes;
    }

    let mut routes = explicit_mirror_routes(url, resource);
    routes.push(official.clone());
    if resource == ResourceClass::CurseForge && !official.is_mirror {
        let mut direct = official;
        direct.proxy = ProxyPolicy::Direct;
        routes.push(direct);
    }
    if mode == crate::state::DownloadSourceMode::Auto {
        order_auto_routes(&mut routes);
    }
    routes
}

fn source_mode_for_resource(
    resource: ResourceClass,
) -> crate::state::DownloadSourceMode {
    let Some(state) = crate::State::get_if_initialized() else {
        return crate::state::DownloadSourceMode::OfficialOnly;
    };

    match resource {
        ResourceClass::Metadata => state.minecraft_metadata_source(),
        ResourceClass::MinecraftAsset
        | ResourceClass::MinecraftLibrary
        | ResourceClass::Loader
        | ResourceClass::Java => state.minecraft_file_source(),
        ResourceClass::Modrinth | ResourceClass::Modpack => {
            state.modrinth_source()
        }
        ResourceClass::CurseForge => state.curseforge_source(),
        ResourceClass::Other => crate::state::DownloadSourceMode::OfficialOnly,
    }
}

fn infer_resource_class(url: &str) -> ResourceClass {
    let Ok(parsed) = Url::parse(url) else {
        return ResourceClass::Other;
    };
    let host = parsed.host_str().unwrap_or_default();
    match host {
        "resources.download.minecraft.net" => ResourceClass::MinecraftAsset,
        "libraries.minecraft.net" => ResourceClass::MinecraftLibrary,
        "maven.minecraftforge.net"
        | "files.minecraftforge.net"
        | "maven.fabricmc.net"
        | "maven.neoforged.net"
        | "meta.fabricmc.net" => ResourceClass::Loader,
        "repo1.maven.org" | "repo.maven.apache.org" => {
            ResourceClass::MinecraftLibrary
        }
        "piston-meta.mojang.com" if parsed.path().contains("java-runtime") => {
            ResourceClass::Java
        }
        "piston-data.mojang.com" if parsed.path().contains("java-runtime") => {
            ResourceClass::Java
        }
        "piston-meta.mojang.com" | "launchermeta.mojang.com" => {
            ResourceClass::Metadata
        }
        "launcher.mojang.com" | "piston-data.mojang.com" => {
            ResourceClass::MinecraftLibrary
        }
        "api.modrinth.com" | "cdn.modrinth.com" => ResourceClass::Modrinth,
        "api.curseforge.com"
        | "edge.forgecdn.net"
        | "media.forgecdn.net"
        | "mediafilez.forgecdn.net" => ResourceClass::CurseForge,
        _ => ResourceClass::Other,
    }
}

pub(crate) fn resolve_download_url(
    url: &str,
    mirrors: DownloadMirrorSettings,
) -> ResolvedDownloadUrl {
    let resource = infer_resource_class(url);
    let enabled = match resource {
        ResourceClass::Metadata
        | ResourceClass::MinecraftAsset
        | ResourceClass::MinecraftLibrary
        | ResourceClass::Loader
        | ResourceClass::Java => mirrors.minecraft,
        ResourceClass::Modrinth | ResourceClass::Modpack => mirrors.modrinth,
        ResourceClass::CurseForge => mirrors.curseforge,
        ResourceClass::Other => false,
    };

    if enabled
        && let Some(route) =
            explicit_mirror_routes(url, resource).into_iter().next()
    {
        return ResolvedDownloadUrl {
            url: route.url,
            is_mirror: true,
        };
    }

    ResolvedDownloadUrl {
        url: url.to_string(),
        is_mirror: false,
    }
}

fn resolve_download_routes(
    url: &str,
    mirrors: DownloadMirrorSettings,
) -> Vec<ResolvedDownloadUrl> {
    let resource = infer_resource_class(url);
    let enabled = match resource {
        ResourceClass::Metadata
        | ResourceClass::MinecraftAsset
        | ResourceClass::MinecraftLibrary
        | ResourceClass::Loader
        | ResourceClass::Java => mirrors.minecraft,
        ResourceClass::Modrinth | ResourceClass::Modpack => mirrors.modrinth,
        ResourceClass::CurseForge => mirrors.curseforge,
        ResourceClass::Other => false,
    };
    if !enabled {
        return vec![ResolvedDownloadUrl {
            url: url.to_string(),
            is_mirror: false,
        }];
    }

    let mut routes = explicit_mirror_routes(url, resource)
        .into_iter()
        .map(|route| ResolvedDownloadUrl {
            url: route.url,
            is_mirror: true,
        })
        .collect::<Vec<_>>();
    routes.push(ResolvedDownloadUrl {
        url: url.to_string(),
        is_mirror: false,
    });
    routes
}

#[derive(Debug, derive_more::Display, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[display(rename_all = "snake_case")]
pub enum DownloadReason {
    Standalone,
    Dependency,
    Modpack,
    Update,
}

#[derive(Debug, Clone, Serialize)]
pub struct DownloadMeta {
    pub reason: DownloadReason,
    pub game_version: String,
    pub loader: String,
    pub dependent_on: Option<String>,
}

impl DownloadMeta {
    pub fn to_header_value(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

#[derive(Debug)]
pub struct IoSemaphore(pub Semaphore);
#[derive(Debug)]
pub struct FetchSemaphore(pub Semaphore);

struct FetchFence {
    inner: Mutex<HashMap<&'static str, FenceInner>>,
}

impl FetchFence {
    pub fn is_blocked(&self, key: &'static str) -> bool {
        self.inner
            .lock()
            .entry(key)
            .or_insert_with(FenceInner::new)
            .is_blocked()
    }

    pub fn record_ok(&self, key: &'static str) {
        self.inner
            .lock()
            .entry(key)
            .or_insert_with(FenceInner::new)
            .record_ok()
    }

    pub fn record_fail(&self, key: &'static str) {
        self.inner
            .lock()
            .entry(key)
            .or_insert_with(FenceInner::new)
            .record_fail()
    }

    pub fn latest_block_minutes(&self) -> u32 {
        let now = Utc::now();

        self.inner
            .lock()
            .values()
            .filter_map(|fence| fence.block_until)
            .filter(|until| *until > now)
            .max()
            .map(|until| {
                let seconds = until.signed_duration_since(now).num_seconds();
                (seconds.max(0) as u32).div_ceil(60).max(1)
            })
            .unwrap_or(1)
    }
}

struct FenceInner {
    failures: VecDeque<DateTime<Utc>>,
    block_until: Option<DateTime<Utc>>,
    block_factor: i32,
}

impl FenceInner {
    const FAILURE_WINDOW: TimeDelta = TimeDelta::minutes(3);
    const FAILURE_THRESHOLD: usize = 4;
    const BLOCK_DURATION_MIN_BASE: TimeDelta = TimeDelta::minutes(2);
    const BLOCK_DURATION_MAX_BASE: TimeDelta = TimeDelta::minutes(5);
    const BLOCK_DURATION_MAX_FACTOR: i32 = 3;

    pub fn new() -> Self {
        Self {
            failures: VecDeque::new(),
            block_until: None,
            block_factor: 0,
        }
    }

    pub fn is_blocked(&mut self) -> bool {
        if let Some(until) = self.block_until {
            if until > Utc::now() {
                return true;
            } else {
                self.block_until = None;
            }
        }

        false
    }

    pub fn record_ok(&mut self) {
        self.prune(Utc::now());
    }

    pub fn record_fail(&mut self) {
        self.prune(Utc::now());
        self.failures.push_back(Utc::now());

        if self.failures.len() >= Self::FAILURE_THRESHOLD {
            self.trigger_block();
        }
    }

    /// Blocks further requests for a random duration between the min and max base durations, scaled by a factor
    /// of how many blocks have been triggered in this session.
    ///
    /// As such, for the first block, the duration will be between 2 and 5 minutes.
    /// - For the second block, between 4 and 10 minutes.
    /// - For the third block and any further blocks, between 6 and 15 minutes.
    fn trigger_block(&mut self) {
        self.block_factor =
            i32::min(self.block_factor + 1, Self::BLOCK_DURATION_MAX_FACTOR);

        let min = Self::BLOCK_DURATION_MIN_BASE
            .checked_mul(self.block_factor)
            .unwrap_or(Self::BLOCK_DURATION_MIN_BASE);
        let max = Self::BLOCK_DURATION_MAX_BASE
            .checked_mul(self.block_factor)
            .unwrap_or(Self::BLOCK_DURATION_MAX_BASE);

        let delta_seconds = (max - min).as_seconds_f64()
            * rand::thread_rng().gen_range(0.0..=1.0);
        let duration =
            min + TimeDelta::milliseconds((delta_seconds * 1000.0) as i64);

        self.block_until = Some(Utc::now() + duration);
    }

    /// Removes all failure points older than the failure window
    fn prune(&mut self, now: DateTime<Utc>) {
        let cutoff = now - Self::FAILURE_WINDOW;

        while let Some(&front) = self.failures.front() {
            if front < cutoff {
                self.failures.pop_front();
            } else {
                break;
            }
        }
    }
}

static GLOBAL_FETCH_FENCE: LazyLock<FetchFence> =
    LazyLock::new(|| FetchFence {
        inner: Mutex::new(HashMap::new()),
    });

fn reqwest_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .connect_timeout(time::Duration::from_secs(15))
        .read_timeout(time::Duration::from_secs(30))
        .tcp_keepalive(Some(time::Duration::from_secs(10)))
        .user_agent(crate::launcher_user_agent())
}

pub static INSECURE_REQWEST_CLIENT: LazyLock<reqwest::Client> =
    LazyLock::new(|| {
        reqwest_client_builder()
            .build()
            .expect("client configuration should be valid")
    });

pub static REQWEST_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest_client_builder()
        .https_only(true)
        .build()
        .expect("client configuration should be valid")
});

const DOWNLOAD_PROGRESS_LOG_INTERVAL: u64 = 8 * 1024 * 1024;
const MODRINTH_CDN_ATTEMPTS: usize = 3;
const MODRINTH_CDN_ATTEMPT_TIMEOUT: time::Duration =
    time::Duration::from_secs(120);

static NO_REDIRECT_REQWEST_CLIENT: LazyLock<reqwest::Client> =
    LazyLock::new(|| {
        reqwest_client_builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("client configuration should be valid")
    });

static DIRECT_REQWEST_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest_client_builder()
        .https_only(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("client configuration should be valid")
});

static DIRECT_FETCH_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest_client_builder()
        .https_only(true)
        .no_proxy()
        .build()
        .expect("client configuration should be valid")
});

const FETCH_RETRY_DELAYS: [time::Duration; 3] = [
    time::Duration::from_millis(250),
    time::Duration::from_millis(750),
    time::Duration::from_secs(2),
];

fn fetch_retry_delay(attempt: usize) -> time::Duration {
    let base = FETCH_RETRY_DELAYS
        .get(attempt.saturating_sub(1))
        .copied()
        .unwrap_or(*FETCH_RETRY_DELAYS.last().unwrap());
    let jitter = rand::thread_rng().gen_range(0.85..=1.15);
    time::Duration::from_secs_f64(base.as_secs_f64() * jitter)
}

fn retry_after(response: &reqwest::Response) -> Option<time::Duration> {
    let value = response.headers().get(header::RETRY_AFTER)?.to_str().ok()?;
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(time::Duration::from_secs(seconds.min(60)));
    }

    let retry_at = DateTime::parse_from_rfc2822(value)
        .ok()?
        .with_timezone(&Utc);
    let seconds = retry_at.signed_duration_since(Utc::now()).num_seconds();
    Some(time::Duration::from_secs(seconds.clamp(0, 60) as u64))
}

fn is_sensitive_header(name: &str) -> bool {
    name.eq_ignore_ascii_case("authorization")
        || name.eq_ignore_ascii_case("proxy-authorization")
        || name.eq_ignore_ascii_case("cookie")
        || name.eq_ignore_ascii_case("x-api-key")
}

fn header_requires_official_only(name: &str) -> bool {
    name.eq_ignore_ascii_case("authorization")
        || name.eq_ignore_ascii_case("proxy-authorization")
        || name.eq_ignore_ascii_case("cookie")
}

fn requires_modrinth_auth(
    method: &Method,
    header: Option<(&str, &str)>,
    uri_path: Option<&str>,
) -> bool {
    if method != Method::GET
        || header.is_some_and(|(name, _)| is_sensitive_header(name))
    {
        return true;
    }

    uri_path.is_some_and(|path| {
        matches!(path, "/v2/user" | "/v3/friends")
            || path.starts_with("/v2/session")
            || path.starts_with("/v3/friend/")
            || path.starts_with("/v3/notification")
    })
}

fn record_route_success(
    route: &DownloadRoute,
    ttfb: time::Duration,
    bytes: u64,
    elapsed: time::Duration,
) {
    ensure_host_health_loaded();
    let Some(host) = route_host(route) else {
        return;
    };
    let mut health_map = HOST_HEALTH.lock();
    let health = health_map.entry(host).or_default();
    health.successes = health.successes.saturating_add(1);
    health.consecutive_failures = 0;
    health.cooldown_until = None;
    let ttfb_ms = ttfb.as_secs_f64() * 1_000.0;
    health.ttfb_ewma_ms = Some(
        health
            .ttfb_ewma_ms
            .map_or(ttfb_ms, |old| old * 0.75 + ttfb_ms * 0.25),
    );
    if !elapsed.is_zero() && bytes > 0 {
        let throughput = bytes as f64 / elapsed.as_secs_f64();
        health.throughput_ewma_bps = Some(
            health
                .throughput_ewma_bps
                .map_or(throughput, |old| old * 0.75 + throughput * 0.25),
        );
    }
    drop(health_map);
    schedule_host_health_persist();
}

fn record_route_failure(route: &DownloadRoute) {
    ensure_host_health_loaded();
    let Some(host) = route_host(route) else {
        return;
    };
    let mut health_map = HOST_HEALTH.lock();
    let health = health_map.entry(host).or_default();
    health.failures = health.failures.saturating_add(1);
    health.consecutive_failures = health.consecutive_failures.saturating_add(1);
    if health.consecutive_failures >= 3 {
        health.cooldown_until =
            Some(Instant::now() + time::Duration::from_secs(120));
    }
    drop(health_map);
    schedule_host_health_persist();
}

fn range_splitting_allowed(route: &DownloadRoute) -> bool {
    ensure_host_health_loaded();
    route_host(route)
        .and_then(|host| {
            HOST_HEALTH
                .lock()
                .get(&host)
                .and_then(|health| health.range_disabled_until)
        })
        .is_none_or(|until| until <= Instant::now())
}

fn disable_range_splitting(route: &DownloadRoute) {
    ensure_host_health_loaded();
    let Some(host) = route_host(route) else {
        return;
    };
    HOST_HEALTH
        .lock()
        .entry(host)
        .or_default()
        .range_disabled_until =
        Some(Instant::now() + time::Duration::from_secs(24 * 60 * 60));
    schedule_host_health_persist();
}

pub type FetchProgressFn<'a> = dyn FnMut(
        u64,
        u64,
    ) -> Pin<Box<dyn Future<Output = crate::Result<()>> + Send + 'a>>
    + Send
    + 'a;

pub type FetchAttemptFn<'a> = dyn FnMut(
        usize,
        usize,
        String,
    ) -> Pin<Box<dyn Future<Output = crate::Result<()>> + Send + 'a>>
    + Send
    + 'a;

fn metadata_hedge_delay(route: &DownloadRoute) -> time::Duration {
    ensure_host_health_loaded();
    let delay_ms = route_host(route)
        .and_then(|host| {
            HOST_HEALTH
                .lock()
                .get(&host)
                .and_then(|health| health.ttfb_ewma_ms)
        })
        .map_or(1_500.0, |ttfb| (ttfb * 2.0).clamp(750.0, 2_500.0));
    time::Duration::from_millis(delay_ms.round() as u64)
}

async fn fetch_public_metadata_route(
    route: &DownloadRoute,
    sha1: Option<&str>,
    semaphore: &FetchSemaphore,
    client: &reqwest::Client,
    response_validator: Option<
        &(dyn Fn(&Bytes) -> crate::Result<()> + Send + Sync),
    >,
) -> crate::Result<Bytes> {
    let client = match route.proxy {
        ProxyPolicy::System => client,
        ProxyPolicy::Direct => &*DIRECT_FETCH_CLIENT,
    };
    let permit = semaphore.0.acquire().await?;
    let request_started = Instant::now();
    let response = match client.get(&route.url).send().await {
        Ok(response) => response,
        Err(error) => {
            drop(permit);
            record_route_failure(route);
            return Err(error.into());
        }
    };
    let ttfb = request_started.elapsed();
    if response.status().is_client_error()
        || response.status().is_server_error()
        || response.status().is_redirection()
    {
        let status = response.status();
        let error = if status.is_redirection() {
            ErrorKind::OtherError(format!(
                "Unexpected metadata redirect from {}",
                route.url
            ))
            .into()
        } else {
            response_status_error(response, &Method::GET, &route.url).await
        };
        drop(permit);
        record_route_failure(route);
        return Err(error);
    }
    let transfer_started = Instant::now();
    let bytes = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(error) => {
            drop(permit);
            record_route_failure(route);
            return Err(error.into());
        }
    };
    drop(permit);
    if let Some(expected) = sha1 {
        let actual = sha1_async(bytes.clone()).await?;
        if !actual.eq_ignore_ascii_case(expected) {
            record_route_failure(route);
            return Err(
                ErrorKind::HashError(expected.to_string(), actual).into()
            );
        }
    }
    if let Some(validate_response) = response_validator {
        if let Err(error) = validate_response(&bytes) {
            record_route_failure(route);
            return Err(error);
        }
    }
    record_route_success(
        route,
        ttfb,
        bytes.len() as u64,
        transfer_started.elapsed(),
    );
    Ok(bytes)
}

async fn fetch_metadata_hedged(
    routes: &[DownloadRoute],
    sha1: Option<&str>,
    semaphore: &FetchSemaphore,
    client: &reqwest::Client,
    response_validator: Option<
        &(dyn Fn(&Bytes) -> crate::Result<()> + Send + Sync),
    >,
) -> (crate::Result<Bytes>, usize) {
    let primary = &routes[0];
    let backup = &routes[1];
    let primary_request = fetch_public_metadata_route(
        primary,
        sha1,
        semaphore,
        client,
        response_validator,
    );
    let backup_request = async {
        tokio::time::sleep(metadata_hedge_delay(primary)).await;
        fetch_public_metadata_route(
            backup,
            sha1,
            semaphore,
            client,
            response_validator,
        )
        .await
    };
    tokio::pin!(primary_request);
    tokio::pin!(backup_request);

    tokio::select! {
        primary_result = &mut primary_request => match primary_result {
            Ok(bytes) => (Ok(bytes), 1),
            Err(primary_error) => {
                let backup_result = fetch_public_metadata_route(
                    backup,
                    sha1,
                    semaphore,
                    client,
                    response_validator,
                ).await;
                match backup_result {
                    Ok(bytes) => (Ok(bytes), 2),
                    Err(_) => (Err(primary_error), 2),
                }
            }
        },
        backup_result = &mut backup_request => match backup_result {
            Ok(bytes) => (Ok(bytes), 2),
            Err(backup_error) => match primary_request.await {
                Ok(bytes) => (Ok(bytes), 2),
                Err(_) => (Err(backup_error), 2),
            },
        },
    }
}

#[tracing::instrument(skip(semaphore))]
pub async fn fetch(
    url: &str,
    sha1: Option<&str>,
    download_meta: Option<&DownloadMeta>,
    uri_path: Option<&'static str>,
    semaphore: &FetchSemaphore,
    exec: impl sqlx::Executor<'_, Database = sqlx::Sqlite>,
) -> crate::Result<Bytes> {
    fetch_advanced(
        Method::GET,
        url,
        sha1,
        None,
        None,
        download_meta,
        None,
        uri_path,
        semaphore,
        exec,
    )
    .await
}

#[tracing::instrument(skip(semaphore))]
pub async fn fetch_with_client(
    url: &str,
    sha1: Option<&str>,
    download_meta: Option<&DownloadMeta>,
    uri_path: Option<&'static str>,
    semaphore: &FetchSemaphore,
    exec: impl sqlx::Executor<'_, Database = sqlx::Sqlite>,
    client: &reqwest::Client,
) -> crate::Result<Bytes> {
    fetch_advanced_with_client(
        Method::GET,
        url,
        sha1,
        None,
        None,
        download_meta,
        None,
        uri_path,
        semaphore,
        exec,
        client,
    )
    .await
}

#[tracing::instrument(skip(semaphore, progress))]
pub async fn fetch_with_client_progress(
    url: &str,
    sha1: Option<&str>,
    download_meta: Option<&DownloadMeta>,
    uri_path: Option<&'static str>,
    semaphore: &FetchSemaphore,
    exec: impl sqlx::Executor<'_, Database = sqlx::Sqlite>,
    client: &reqwest::Client,
    progress: Option<&mut FetchProgressFn<'_>>,
) -> crate::Result<Bytes> {
    fetch_advanced_with_client_and_progress(
        Method::GET,
        url,
        sha1,
        None,
        None,
        download_meta,
        None,
        uri_path,
        semaphore,
        exec,
        client,
        progress,
        None,
        None,
        ARTIFACT_ATTEMPT_BUDGET,
    )
    .await
}

#[tracing::instrument(skip(json_body, semaphore))]
pub async fn fetch_json<T>(
    method: Method,
    url: &str,
    sha1: Option<&str>,
    json_body: Option<serde_json::Value>,
    uri_path: Option<&'static str>,
    semaphore: &FetchSemaphore,
    exec: impl sqlx::Executor<'_, Database = sqlx::Sqlite>,
) -> crate::Result<T>
where
    T: DeserializeOwned,
{
    let validate_json = |bytes: &Bytes| -> crate::Result<()> {
        serde_json::from_slice::<T>(bytes)
            .map(|_| ())
            .map_err(Into::into)
    };
    let result = fetch_advanced_with_client_and_progress(
        method,
        url,
        sha1,
        json_body,
        None,
        None,
        None,
        uri_path,
        semaphore,
        exec,
        &INSECURE_REQWEST_CLIENT,
        None,
        None,
        Some(&validate_json),
        ARTIFACT_ATTEMPT_BUDGET,
    )
    .await?;
    Ok(serde_json::from_slice(&result)?)
}

/// Downloads a file with retry and checksum functionality, and a specific
/// [`reqwest::Client`].
#[tracing::instrument(skip(json_body, semaphore))]
#[allow(clippy::too_many_arguments)]
pub async fn fetch_advanced(
    method: Method,
    url: &str,
    sha1: Option<&str>,
    json_body: Option<serde_json::Value>,
    header: Option<(&str, &str)>,
    download_meta: Option<&DownloadMeta>,
    loading_bar: Option<(&LoadingBarId, f64)>,
    uri_path: Option<&'static str>,
    semaphore: &FetchSemaphore,
    exec: impl sqlx::Executor<'_, Database = sqlx::Sqlite>,
) -> crate::Result<Bytes> {
    fetch_advanced_with_client(
        method,
        url,
        sha1,
        json_body,
        header,
        download_meta,
        loading_bar,
        uri_path,
        semaphore,
        exec,
        &INSECURE_REQWEST_CLIENT,
    )
    .await
}

#[tracing::instrument(skip(json_body, semaphore, progress))]
#[allow(clippy::too_many_arguments)]
pub async fn fetch_advanced_with_progress(
    method: Method,
    url: &str,
    sha1: Option<&str>,
    json_body: Option<serde_json::Value>,
    header: Option<(&str, &str)>,
    download_meta: Option<&DownloadMeta>,
    loading_bar: Option<(&LoadingBarId, f64)>,
    uri_path: Option<&'static str>,
    semaphore: &FetchSemaphore,
    exec: impl sqlx::Executor<'_, Database = sqlx::Sqlite>,
    progress: Option<&mut FetchProgressFn<'_>>,
) -> crate::Result<Bytes> {
    fetch_advanced_with_client_and_progress(
        method,
        url,
        sha1,
        json_body,
        header,
        download_meta,
        loading_bar,
        uri_path,
        semaphore,
        exec,
        &INSECURE_REQWEST_CLIENT,
        progress,
        None,
        None,
        ARTIFACT_ATTEMPT_BUDGET,
    )
    .await
}

/// Downloads a file with retry and checksum functionality
#[tracing::instrument(skip(json_body, semaphore))]
#[allow(clippy::too_many_arguments)]
pub async fn fetch_advanced_with_client(
    method: Method,
    url: &str,
    sha1: Option<&str>,
    json_body: Option<serde_json::Value>,
    header: Option<(&str, &str)>,
    download_meta: Option<&DownloadMeta>,
    loading_bar: Option<(&LoadingBarId, f64)>,
    uri_path: Option<&'static str>,
    semaphore: &FetchSemaphore,
    exec: impl sqlx::Executor<'_, Database = sqlx::Sqlite>,
    client: &reqwest::Client,
) -> crate::Result<Bytes> {
    fetch_advanced_with_client_and_progress(
        method,
        url,
        sha1,
        json_body,
        header,
        download_meta,
        loading_bar,
        uri_path,
        semaphore,
        exec,
        client,
        None,
        None,
        None,
        ARTIFACT_ATTEMPT_BUDGET,
    )
    .await
}

#[tracing::instrument(skip(
    json_body,
    semaphore,
    client,
    progress,
    attempt_reporter,
    response_validator
))]
#[allow(clippy::too_many_arguments)]
async fn fetch_advanced_with_client_and_progress(
    method: Method,
    url: &str,
    sha1: Option<&str>,
    json_body: Option<serde_json::Value>,
    header: Option<(&str, &str)>,
    download_meta: Option<&DownloadMeta>,
    loading_bar: Option<(&LoadingBarId, f64)>,
    uri_path: Option<&'static str>,
    semaphore: &FetchSemaphore,
    exec: impl sqlx::Executor<'_, Database = sqlx::Sqlite>,
    client: &reqwest::Client,
    mut progress: Option<&mut FetchProgressFn<'_>>,
    mut attempt_reporter: Option<&mut FetchAttemptFn<'_>>,
    response_validator: Option<
        &(dyn Fn(&Bytes) -> crate::Result<()> + Send + Sync),
    >,
    attempt_budget: usize,
) -> crate::Result<Bytes> {
    let resource = infer_resource_class(url);
    let mode = source_mode_for_resource(resource);
    let mut request_routes = resolve_download_routes_for(url, resource, mode);
    if sha1.is_none() {
        request_routes
            .retain(|route| route.source != DownloadRouteSource::TencentMaven);
    }
    let modrinth_request_kind = modrinth_request_kind(url);
    let is_mrpack_download =
        modrinth_request_kind == Some("CDN") && is_mrpack_url(url);
    let is_api_url = url.starts_with(env!("MODRINTH_API_URL"))
        || url.starts_with(env!("MODRINTH_API_URL_V3"));
    let requires_auth =
        is_api_url && requires_modrinth_auth(&method, header, uri_path);
    let creds = if requires_auth
        && header.as_ref().is_none_or(|x| !is_sensitive_header(x.0))
    {
        crate::state::ModrinthCredentials::get_active(exec).await?
    } else {
        None
    };
    if method != Method::GET
        || header
            .as_ref()
            .is_some_and(|header| header_requires_official_only(header.0))
        || requires_auth
    {
        request_routes.retain(|route| !route.is_mirror);
    }
    if request_routes.is_empty() {
        request_routes.push(official_route(url, resource));
    }

    let mut total_attempts = 0;
    let mut last_error = None;
    if resource == ResourceClass::Metadata
        && method == Method::GET
        && json_body.is_none()
        && header.is_none()
        && download_meta.is_none()
        && loading_bar.is_none()
        && progress.is_none()
        && creds.is_none()
        && request_routes.len() >= 2
        && attempt_budget >= 2
    {
        let (result, attempts) = fetch_metadata_hedged(
            &request_routes,
            sha1,
            semaphore,
            client,
            response_validator,
        )
        .await;
        total_attempts = attempts;
        match result {
            Ok(bytes) => return Ok(bytes),
            Err(error) => last_error = Some(error),
        }
    }

    for (route_index, route) in request_routes.iter().enumerate() {
        let request_url = &route.url;
        let is_mirror = route.is_mirror;
        let route_source = route.source;
        let has_next_route = route_index + 1 < request_routes.len();
        let fence_key = if is_api_url && !is_mirror {
            uri_path
        } else {
            None
        };
        let download_meta_header = (!is_mirror
            && is_official_modrinth_download_url(request_url))
        .then(|| {
            download_meta.map(|m| {
                (DOWNLOAD_META_HEADER.to_string(), m.to_header_value())
            })
        })
        .flatten();

        let max_attempts = if modrinth_request_kind == Some("CDN") {
            if is_mirror { 1 } else { MODRINTH_CDN_ATTEMPTS }
        } else {
            attempt_budget
        };
        let mut retried_server_error = false;
        let mut route_attempts = 0;
        while total_attempts < attempt_budget {
            let remaining_routes = request_routes.len() - route_index - 1;
            route_attempts += 1;
            let attempt = route_attempts;
            let has_more_attempts = attempt < max_attempts
                && total_attempts + remaining_routes < attempt_budget;
            if let Some(fence_key) = fence_key
                && GLOBAL_FETCH_FENCE.is_blocked(fence_key)
            {
                return Err(ErrorKind::ApiIsDownError(
                    GLOBAL_FETCH_FENCE.latest_block_minutes(),
                )
                .into());
            }
            total_attempts += 1;

            let started = time::Instant::now();
            if let Some(request_kind) = modrinth_request_kind {
                tracing::info!(
                    source = ?route_source,
                    request_kind,
                    method = %method,
                    url = request_url,
                    route = route_index + 1,
                    attempt,
                    max_attempts,
                    "Attempting Modrinth request"
                );
            }

            if modrinth_request_kind == Some("CDN")
                && !is_mirror
                && let Some(attempt_reporter) = attempt_reporter.as_mut()
            {
                attempt_reporter(attempt, max_attempts, request_url.clone())
                    .await?;
            }

            let protected_headers = creds.is_some()
                || download_meta_header.is_some()
                || header.is_some_and(|header| is_sensitive_header(header.0));
            let route_client = match (route.proxy, protected_headers) {
                (ProxyPolicy::System, false)
                    if is_mirror && modrinth_request_kind.is_some() =>
                {
                    &*NO_REDIRECT_REQWEST_CLIENT
                }
                (ProxyPolicy::System, false) => client,
                (ProxyPolicy::System, true) => &*NO_REDIRECT_REQWEST_CLIENT,
                (ProxyPolicy::Direct, false) => &*DIRECT_FETCH_CLIENT,
                (ProxyPolicy::Direct, true) => &*DIRECT_REQWEST_CLIENT,
            };
            let mut req = route_client.request(method.clone(), request_url);
            if modrinth_request_kind == Some("CDN") && !is_mrpack_download {
                req = req.timeout(MODRINTH_CDN_ATTEMPT_TIMEOUT);
            }

            if let Some(body) = json_body.clone() {
                req = req.json(&body);
            }

            if let Some(header) = header
                && (route.allow_sensitive_headers
                    || !is_sensitive_header(header.0))
            {
                req = req.header(header.0, header.1);
            }

            if route.allow_sensitive_headers
                && let Some(ref creds) = creds
            {
                req = req.header("Authorization", &creds.session);
            }

            if let Some((name, value)) = &download_meta_header {
                tracing::debug!("Sending download analytics: {value}");
                req = req.header(name.as_str(), value.as_str());
            }

            let permit = semaphore.0.acquire().await?;
            let request_started = Instant::now();
            let result = req.send().await;
            let ttfb = request_started.elapsed();
            match result {
                Ok(resp) => {
                    let status = resp.status();
                    let retry_after = retry_after(&resp);
                    if status.is_redirection() {
                        if is_mirror
                            && has_next_route
                            && modrinth_request_kind.is_some()
                        {
                            let status = resp.status();
                            let redirect_url = resp
                                .headers()
                                .get(reqwest::header::LOCATION)
                                .and_then(|value| value.to_str().ok())
                                .map(str::to_string);
                            let cache_status = resp
                                .headers()
                                .get("eo-cache-status")
                                .and_then(|value| value.to_str().ok())
                                .unwrap_or("unknown");
                            let redirects_to_official =
                                is_official_modrinth_cdn_redirect(
                                    redirect_url.as_deref(),
                                );
                            if redirects_to_official {
                                tracing::warn!(
                                    mirror_status = "cache_miss",
                                    source = ?route_source,
                                    mirror_url = request_url,
                                    redirect_url = redirect_url.as_deref().unwrap_or("<missing>"),
                                    cache_status,
                                    status = status.as_u16(),
                                    elapsed_ms = started.elapsed().as_millis(),
                                    "Modrinth mirror redirected to official CDN; falling back to official source"
                                );
                            } else {
                                tracing::warn!(
                                    mirror_status = "redirect_unresolved",
                                    source = ?route_source,
                                    mirror_url = request_url,
                                    redirect_url = redirect_url.as_deref().unwrap_or("<missing>"),
                                    cache_status,
                                    status = status.as_u16(),
                                    elapsed_ms = started.elapsed().as_millis(),
                                    "Modrinth mirror returned an unresolved redirect; falling back to official source"
                                );
                            }
                        }
                        drop(permit);
                        record_route_failure(route);
                        last_error = Some(
							ErrorKind::OtherError(format!(
								"Refusing to automatically forward protected headers while redirecting {request_url}"
							))
							.into(),
						);
                        break;
                    }
                    if status.is_client_error() || status.is_server_error() {
                        if let Some(fence_key) = fence_key {
                            if status.is_server_error() {
                                GLOBAL_FETCH_FENCE.record_fail(fence_key);
                            }
                        }
                        record_route_failure(route);
                        let backup_error =
                            resp.error_for_status_ref().unwrap_err();
                        let route_error: crate::Error = if let Ok(mut error) =
                            resp.json::<LabrinthError>().await
                        {
                            error.status = Some(status.as_u16());
                            error.method = Some(method.as_str().to_string());
                            error.url = Some(request_url.to_string());
                            error.route = uri_path.map(str::to_string);
                            ErrorKind::LabrinthError(error).into()
                        } else {
                            backup_error.into()
                        };
                        let route_error_message = route_error.to_string();
                        drop(permit);
                        last_error = Some(route_error);

                        if status == StatusCode::TOO_MANY_REQUESTS
                            && !has_next_route
                            && has_more_attempts
                        {
                            tokio::time::sleep(retry_after.unwrap_or_else(
                                || fetch_retry_delay(total_attempts),
                            ))
                            .await;
                            continue;
                        }

                        if status.is_server_error()
                            && !retried_server_error
                            && has_more_attempts
                            && total_attempts + remaining_routes
                                < attempt_budget
                        {
                            retried_server_error = true;
                            tokio::time::sleep(fetch_retry_delay(
                                total_attempts,
                            ))
                            .await;
                            continue;
                        }

                        if has_next_route {
                            if modrinth_request_kind.is_some() {
                                tracing::warn!(
                                    source = ?route_source,
                                    url = request_url,
                                    status = status.as_u16(),
                                    elapsed_ms = started.elapsed().as_millis(),
                                    error = %route_error_message,
                                    "Modrinth mirror failed; falling back to official source"
                                );
                            } else {
                                tracing::warn!(
                                    url = request_url,
                                    status = status.as_u16(),
                                    error = %route_error_message,
                                    "Mirror request failed; falling back to official source"
                                );
                            }
                            break;
                        }
                        if modrinth_request_kind.is_some() {
                            tracing::warn!(
                                source = ?route_source,
                                url = request_url,
                                status = status.as_u16(),
                                elapsed_ms = started.elapsed().as_millis(),
                                error = %route_error_message,
                                "Modrinth official request failed"
                            );
                        }
                        break;
                    }

                    let response_url = resp.url().to_string();
                    if is_mirror && modrinth_request_kind == Some("CDN") {
                        let cache_status = resp
                            .headers()
                            .get("eo-cache-status")
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or("unknown");
                        tracing::info!(
                            mirror_status = "cache_hit",
                            source = ?route_source,
                            mirror_url = request_url,
                            final_url = %response_url,
                            cache_status,
                            status = resp.status().as_u16(),
                            elapsed_ms = started.elapsed().as_millis(),
                            "Modrinth mirror resolved cached file"
                        );
                    }
                    let transfer_started = Instant::now();
                    let bytes: eyre::Result<Bytes> = if loading_bar.is_some()
                        || progress.is_some()
                    {
                        let total_size = resp.content_length().unwrap_or(0);
                        let mut stream = resp.bytes_stream();

                        async {
                            let mut bytes = Vec::new();
                            let mut downloaded = 0_u64;
                            let mut next_progress_log =
                                DOWNLOAD_PROGRESS_LOG_INTERVAL;

                            while let Some(item) = stream.next().await {
                                let chunk = item.wrap_err_with(|| {
									eyre!(
										"failed to read response body from {request_url}"
									)
								})?;

                                downloaded += chunk.len() as u64;
                                bytes.extend_from_slice(&chunk);

                                if modrinth_request_kind == Some("CDN")
                                    && downloaded >= next_progress_log
                                {
                                    tracing::info!(
                                        source = ?route_source,
                                        attempt,
                                        url = request_url,
                                        final_url = %response_url,
                                        downloaded_bytes = downloaded,
                                        expected_bytes = total_size,
                                        "Modrinth CDN download progress"
                                    );
                                    while next_progress_log <= downloaded {
                                        next_progress_log = next_progress_log
                                            .saturating_add(
                                                DOWNLOAD_PROGRESS_LOG_INTERVAL,
                                            );
                                    }
                                }

                                if total_size > 0
                                    && let Some((bar, total)) = &loading_bar
                                {
                                    emit_loading(
                                        bar,
                                        (chunk.len() as f64
                                            / total_size as f64)
                                            * total,
                                        None,
                                    )?;
                                }

                                if let Some(progress) = progress.as_mut() {
                                    progress(downloaded, total_size).await?;
                                }
                            }

                            Ok(Bytes::from(bytes))
                        }
                        .await
                    } else {
                        resp.bytes().await.wrap_err_with(|| {
                            eyre!(
                                "failed to read response body from {request_url}"
                            )
						})
                    };
                    drop(permit);

                    if let Ok(bytes) = bytes {
                        if let Some(sha1) = sha1 {
                            let hash = sha1_async(bytes.clone()).await?;
                            if &*hash != sha1 {
                                record_route_failure(route);
                                let route_error: crate::Error =
                                    ErrorKind::HashError(
                                        sha1.to_string(),
                                        hash,
                                    )
                                    .into();
                                last_error = Some(route_error);
                                if !has_next_route && has_more_attempts {
                                    if modrinth_request_kind.is_some() {
                                        tracing::warn!(
                                            source = ?route_source,
                                            url = request_url,
                                            attempt,
                                            max_attempts,
                                            elapsed_ms = started.elapsed().as_millis(),
                                            "Modrinth checksum validation failed; retrying"
                                        );
                                    }
                                    tokio::time::sleep(fetch_retry_delay(
                                        total_attempts,
                                    ))
                                    .await;
                                    continue;
                                }
                                break;
                            }
                        }

                        if let Some(validate_response) = response_validator
                            && let Err(error) = validate_response(&bytes)
                        {
                            record_route_failure(route);
                            if has_next_route {
                                tracing::warn!(
                                    url = request_url,
                                    error = %error,
                                    "Download route returned incompatible data; trying the next source"
                                );
                                last_error = Some(error);
                                break;
                            }
                            return Err(error);
                        }

                        tracing::trace!("Done downloading URL {request_url}");
                        if let Some(request_kind) = modrinth_request_kind {
                            tracing::info!(
                                source = ?route_source,
                                request_kind,
                                url = request_url,
                                final_url = %response_url,
                                attempt,
                                bytes = bytes.len(),
                                elapsed_ms = started.elapsed().as_millis(),
                                "Completed Modrinth request"
                            );
                        }

                        if let Some(fence_key) = fence_key {
                            GLOBAL_FETCH_FENCE.record_ok(fence_key);
                        }
                        record_route_success(
                            route,
                            ttfb,
                            bytes.len() as u64,
                            transfer_started.elapsed(),
                        );

                        return Ok(bytes);
                    } else if let Err(err) = bytes {
                        record_route_failure(route);
                        let error_message = err.to_string();
                        last_error = Some(err.into());
                        if has_next_route {
                            if modrinth_request_kind.is_some() {
                                tracing::warn!(
                                    source = ?route_source,
                                    url = request_url,
                                    elapsed_ms = started.elapsed().as_millis(),
                                    error = %error_message,
                                    "Modrinth mirror response failed; falling back to official source"
                                );
                            } else {
                                tracing::warn!(
                                    url = request_url,
                                    error = %error_message,
                                    "Mirror response failed; falling back to official source"
                                );
                            }
                            break;
                        }
                        if has_more_attempts {
                            if modrinth_request_kind.is_some() {
                                tracing::warn!(
                                    source = ?route_source,
                                    url = request_url,
                                    attempt,
                                    max_attempts,
                                    elapsed_ms = started.elapsed().as_millis(),
                                    error = %error_message,
                                    "Modrinth response body failed; retrying"
                                );
                            }
                            tokio::time::sleep(fetch_retry_delay(
                                total_attempts,
                            ))
                            .await;
                            continue;
                        }
                        break;
                    }
                }
                Err(err) => {
                    drop(permit);
                    record_route_failure(route);
                    let error_message = err.to_string();
                    last_error = Some(err.into());
                    if has_next_route {
                        if modrinth_request_kind.is_some() {
                            tracing::warn!(
                                source = ?route_source,
                                url = request_url,
                                elapsed_ms = started.elapsed().as_millis(),
                                error = %error_message,
                                "Modrinth mirror connection failed; falling back to official source"
                            );
                        } else {
                            tracing::warn!(
                                url = request_url,
                                error = %error_message,
                                "Mirror connection failed; falling back to official source"
                            );
                        }
                        break;
                    }
                    if has_more_attempts {
                        if modrinth_request_kind.is_some() {
                            tracing::warn!(
                                source = ?route_source,
                                url = request_url,
                                attempt,
                                max_attempts,
                                elapsed_ms = started.elapsed().as_millis(),
                                error = %error_message,
                                "Modrinth connection failed; retrying"
                            );
                        } else {
                            tracing::debug!(
                                attempt,
                                url = request_url,
                                error = %error_message,
                                "Fetch failed; retrying"
                            );
                        }
                        tokio::time::sleep(fetch_retry_delay(total_attempts))
                            .await;
                        continue;
                    }
                    break;
                }
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        ErrorKind::OtherError(format!(
            "Unable to download {url} from any source"
        ))
        .into()
    }))
}

#[derive(Default)]
struct IntegrityHashers {
    sha1: Option<sha1_smol::Sha1>,
    sha512: Option<Sha512>,
    sha256: Option<Sha256>,
    md5: Option<md5::Context>,
}

#[derive(Default)]
struct ComputedIntegrity {
    size: u64,
    sha1: Option<String>,
    sha512: Option<String>,
    sha256: Option<String>,
    md5: Option<String>,
}

impl IntegrityHashers {
    fn new(integrity: &Integrity) -> Self {
        Self {
            sha1: integrity.sha1.as_ref().map(|_| sha1_smol::Sha1::new()),
            sha512: integrity.sha512.as_ref().map(|_| Sha512::new()),
            sha256: integrity.sha256.as_ref().map(|_| Sha256::new()),
            md5: integrity.md5.as_ref().map(|_| md5::Context::new()),
        }
    }

    fn update(&mut self, bytes: &[u8]) {
        if let Some(hasher) = &mut self.sha1 {
            hasher.update(bytes);
        }
        if let Some(hasher) = &mut self.sha512 {
            hasher.update(bytes);
        }
        if let Some(hasher) = &mut self.sha256 {
            hasher.update(bytes);
        }
        if let Some(hasher) = &mut self.md5 {
            hasher.consume(bytes);
        }
    }

    fn finish(self, size: u64) -> ComputedIntegrity {
        ComputedIntegrity {
            size,
            sha1: self.sha1.map(|hasher| hasher.digest().to_string()),
            sha512: self
                .sha512
                .map(|hasher| format!("{:x}", hasher.finalize())),
            sha256: self
                .sha256
                .map(|hasher| format!("{:x}", hasher.finalize())),
            md5: self.md5.map(|hasher| format!("{:x}", hasher.finalize())),
        }
    }
}

fn suffixed_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn resume_metadata_matches(
    metadata: &ResumeMetadata,
    route: &DownloadRoute,
    integrity: &Integrity,
) -> bool {
    metadata.url == route.url
        && metadata.expected_size == integrity.size
        && metadata.sha1 == integrity.sha1
        && metadata.sha512 == integrity.sha512
        && metadata.sha256 == integrity.sha256
        && metadata.md5 == integrity.md5
        && (metadata.etag.is_some() || metadata.last_modified.is_some())
}

async fn read_resume_metadata(path: &Path) -> Option<ResumeMetadata> {
    let bytes = tokio::fs::read(path).await.ok()?;
    serde_json::from_slice(&bytes).ok()
}

async fn write_resume_metadata(
    path: &Path,
    metadata: &ResumeMetadata,
) -> crate::Result<()> {
    let bytes = serde_json::to_vec(metadata)?;
    tokio::fs::write(path, bytes)
        .await
        .map_err(|error| IOError::with_path(error, path))?;
    Ok(())
}

async fn remove_if_exists(path: &Path) -> crate::Result<()> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(IOError::with_path(error, path).into()),
    }
}

async fn compute_file_integrity(
    path: &Path,
    integrity: &Integrity,
) -> crate::Result<ComputedIntegrity> {
    let mut file = File::open(path)
        .await
        .map_err(|error| IOError::with_path(error, path))?;
    let mut hashers = IntegrityHashers::new(integrity);
    let mut size = 0;
    let mut buffer = vec![0_u8; 256 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|error| IOError::with_path(error, path))?;
        if read == 0 {
            break;
        }
        hashers.update(&buffer[..read]);
        size += read as u64;
    }
    Ok(hashers.finish(size))
}

fn verify_computed_integrity(
    expected: &Integrity,
    actual: &ComputedIntegrity,
) -> crate::Result<()> {
    if let Some(size) = expected.size
        && actual.size != size
    {
        return Err(ErrorKind::OtherError(format!(
            "Incorrect size for download: {size} != {}",
            actual.size
        ))
        .into());
    }

    let checks = [
        ("sha1", expected.sha1.as_ref(), actual.sha1.as_ref()),
        ("sha512", expected.sha512.as_ref(), actual.sha512.as_ref()),
        ("sha256", expected.sha256.as_ref(), actual.sha256.as_ref()),
        ("md5", expected.md5.as_ref(), actual.md5.as_ref()),
    ];
    for (algorithm, expected, actual) in checks {
        if let Some(expected) = expected
            && actual
                .is_none_or(|actual| !actual.eq_ignore_ascii_case(expected))
        {
            return Err(ErrorKind::OtherError(format!(
                "Incorrect {algorithm} hash for download: {expected} != {}",
                actual.map(String::as_str).unwrap_or("not computed")
            ))
            .into());
        }
    }
    Ok(())
}

async fn validate_file_content(
    path: &Path,
    validation: ContentValidation,
) -> crate::Result<()> {
    if validation == ContentValidation::None {
        return Ok(());
    }
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || -> crate::Result<()> {
        let file = std::fs::File::open(&path)
            .map_err(|error| IOError::with_path(error, &path))?;
        match validation {
            ContentValidation::None => {}
            ContentValidation::Json => {
                serde_json::from_reader::<_, serde_json::Value>(file)?;
            }
            ContentValidation::Jar => {
                zip::ZipArchive::new(file).map_err(|error| {
                    ErrorKind::OtherError(format!(
                        "Invalid JAR archive {}: {error}",
                        path.display()
                    ))
                })?;
            }
        }
        Ok(())
    })
    .await??;
    Ok(())
}

async fn verify_file(path: &Path, integrity: &Integrity) -> crate::Result<u64> {
    let computed = compute_file_integrity(path, integrity).await?;
    verify_computed_integrity(integrity, &computed)?;
    validate_file_content(path, integrity.content).await?;
    Ok(computed.size)
}

fn integrity_cache_key(integrity: &Integrity) -> Option<(&'static str, &str)> {
    if let Some(hash) = &integrity.sha512 {
        Some(("sha512", hash))
    } else if let Some(hash) = &integrity.sha256 {
        Some(("sha256", hash))
    } else if let Some(hash) = &integrity.sha1 {
        Some(("sha1", hash))
    } else if let Some(hash) = &integrity.md5 {
        Some(("md5", hash))
    } else {
        None
    }
}

fn download_lock_key(destination: &Path, integrity: &Integrity) -> String {
    integrity_cache_key(integrity).map_or_else(
        || format!("path:{}", destination.display()),
        |(algorithm, hash)| {
            format!("{algorithm}:{}", hash.to_ascii_lowercase())
        },
    )
}

fn in_flight_download_lock(key: String) -> Arc<AsyncMutex<()>> {
    use dashmap::mapref::entry::Entry;

    if IN_FLIGHT_DOWNLOADS.len() > 4_096 {
        IN_FLIGHT_DOWNLOADS.retain(|_, lock| lock.strong_count() > 0);
    }
    match IN_FLIGHT_DOWNLOADS.entry(key) {
        Entry::Occupied(mut entry) => {
            if let Some(lock) = entry.get().upgrade() {
                lock
            } else {
                let lock = Arc::new(AsyncMutex::new(()));
                entry.insert(Arc::downgrade(&lock));
                lock
            }
        }
        Entry::Vacant(entry) => {
            let lock = Arc::new(AsyncMutex::new(()));
            entry.insert(Arc::downgrade(&lock));
            lock
        }
    }
}

fn cas_path(integrity: &Integrity) -> Option<PathBuf> {
    let (algorithm, hash) = integrity_cache_key(integrity)?;
    let state = crate::State::get_if_initialized()?;
    let normalized = hash.to_ascii_lowercase();
    let prefix = normalized.get(..2).unwrap_or("00");
    Some(
        state
            .directories
            .caches_dir()
            .join("downloads")
            .join(algorithm)
            .join(prefix)
            .join(normalized),
    )
}

async fn hard_link_or_copy(
    source: &Path,
    destination: &Path,
    allow_hard_link: bool,
) -> crate::Result<()> {
    if let Some(parent) = destination.parent() {
        io::create_dir_all(parent).await?;
    }
    remove_if_exists(destination).await?;
    let hard_link_result = if allow_hard_link {
        tokio::fs::hard_link(source, destination).await
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "hard links disabled for mutable content",
        ))
    };
    match hard_link_result {
        Ok(()) => Ok(()),
        Err(_) => {
            tokio::fs::copy(source, destination)
                .await
                .map_err(|error| IOError::with_path(error, destination))?;
            Ok(())
        }
    }
}

async fn materialize_cached_download(
    cache_path: &Path,
    destination: &Path,
    allow_hard_link: bool,
) -> crate::Result<()> {
    let temporary = suffixed_path(destination, ".cas.part");
    hard_link_or_copy(cache_path, &temporary, allow_hard_link).await?;
    if tokio::fs::try_exists(destination)
        .await
        .map_err(|error| IOError::with_path(error, destination))?
    {
        remove_if_exists(destination).await?;
    }
    tokio::fs::rename(&temporary, destination)
        .await
        .map_err(|error| IOError::with_path(error, destination))?;
    Ok(())
}

async fn cache_completed_download(
    destination: &Path,
    integrity: &Integrity,
    allow_hard_link: bool,
) -> crate::Result<()> {
    let Some(cache_path) = cas_path(integrity) else {
        return Ok(());
    };
    if tokio::fs::try_exists(&cache_path)
        .await
        .map_err(|error| IOError::with_path(error, &cache_path))?
    {
        return Ok(());
    }
    let temporary = suffixed_path(&cache_path, ".part");
    hard_link_or_copy(destination, &temporary, allow_hard_link).await?;
    match tokio::fs::rename(&temporary, &cache_path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            remove_if_exists(&temporary).await
        }
        Err(error) => Err(IOError::with_path(error, &cache_path).into()),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ParsedContentRange {
    start: u64,
    end: u64,
    total: u64,
}

fn parse_content_range(
    response: &reqwest::Response,
) -> Option<ParsedContentRange> {
    let value = response
        .headers()
        .get(header::CONTENT_RANGE)?
        .to_str()
        .ok()?
        .strip_prefix("bytes ")?;
    let (range, total) = value.split_once('/')?;
    let (start, end) = range.split_once('-')?;
    Some(ParsedContentRange {
        start: start.parse().ok()?,
        end: end.parse().ok()?,
        total: total.parse().ok()?,
    })
}

fn content_range_starts_at(response: &reqwest::Response, start: u64) -> bool {
    parse_content_range(response).is_some_and(|range| range.start == start)
}

async fn response_status_error(
    response: reqwest::Response,
    method: &Method,
    request_url: &str,
) -> crate::Error {
    let status = response.status();
    let backup_error = response.error_for_status_ref().unwrap_err();
    if let Ok(mut error) = response.json::<LabrinthError>().await {
        error.status = Some(status.as_u16());
        error.method = Some(method.as_str().to_string());
        error.url = Some(request_url.to_string());
        ErrorKind::LabrinthError(error).into()
    } else {
        backup_error.into()
    }
}

async fn finalize_download(
    part_path: &Path,
    metadata_path: &Path,
    destination: &Path,
) -> crate::Result<()> {
    if tokio::fs::try_exists(destination)
        .await
        .map_err(|error| IOError::with_path(error, destination))?
    {
        remove_if_exists(destination).await?;
    }
    tokio::fs::rename(part_path, destination)
        .await
        .map_err(|error| IOError::with_path(error, destination))?;
    remove_if_exists(metadata_path).await?;
    Ok(())
}

fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

#[allow(clippy::too_many_arguments)]
async fn send_path_request_with_clients(
    route: &DownloadRoute,
    custom_header: Option<&(String, String)>,
    credentials: Option<&crate::state::ModrinthCredentials>,
    download_meta: Option<&DownloadMeta>,
    range_start: Option<u64>,
    range_end: Option<u64>,
    resume_metadata: Option<&ResumeMetadata>,
    system_client: &reqwest::Client,
    direct_client: &reqwest::Client,
) -> crate::Result<(reqwest::Response, String)> {
    let client = match route.proxy {
        ProxyPolicy::System => system_client,
        ProxyPolicy::Direct => direct_client,
    };
    let original = Url::parse(&route.url)?;
    let mut current = original.clone();
    for redirect_count in 0..=5 {
        let same_as_original = same_origin(&original, &current);
        let allow_sensitive = route.allow_sensitive_headers && same_as_original;
        let mut request = client.get(current.clone());
        if let Some((name, value)) = custom_header
            && (allow_sensitive || !is_sensitive_header(name))
            && (!name.eq_ignore_ascii_case("x-api-key")
                || original.host_str() == Some("api.curseforge.com"))
        {
            request = request.header(name, value);
        }
        if allow_sensitive && let Some(credentials) = credentials {
            request = request.header("Authorization", &credentials.session);
        }
        if !route.is_mirror
            && same_as_original
            && is_official_modrinth_download_url(original.as_str())
            && let Some(download_meta) = download_meta
        {
            request = request
                .header(DOWNLOAD_META_HEADER, download_meta.to_header_value());
        }
        if let Some(range_start) = range_start {
            let range = range_end.map_or_else(
                || format!("bytes={range_start}-"),
                |end| format!("bytes={range_start}-{end}"),
            );
            request = request.header(header::RANGE, range);
            if let Some(metadata) = resume_metadata {
                if let Some(etag) = &metadata.etag {
                    request = request.header(header::IF_RANGE, etag);
                } else if let Some(last_modified) = &metadata.last_modified {
                    request = request.header(header::IF_RANGE, last_modified);
                }
            }
        }

        let response = request.send().await?;
        if !response.status().is_redirection() {
            return Ok((response, current.into()));
        }
        if redirect_count == 5 {
            return Err(ErrorKind::OtherError(format!(
                "Too many redirects while downloading {}",
                route.url
            ))
            .into());
        }
        let location = response
            .headers()
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| {
                ErrorKind::OtherError(format!(
                    "Redirect from {} did not include a valid Location header",
                    current
                ))
            })?;
        if !is_safe_redirect_location(location) {
            return Err(ErrorKind::OtherError(format!(
                "Redirect from {current} included an unsafe Location header"
            ))
            .into());
        }
        let next = current.join(location)?;
        if next.scheme() != "https" {
            return Err(ErrorKind::OtherError(format!(
                "Refusing insecure redirect from {current} to {next}"
            ))
            .into());
        }
        current = next;
    }
    unreachable!()
}

#[allow(clippy::too_many_arguments)]
async fn send_path_request(
    route: &DownloadRoute,
    custom_header: Option<&(String, String)>,
    credentials: Option<&crate::state::ModrinthCredentials>,
    download_meta: Option<&DownloadMeta>,
    range_start: Option<u64>,
    range_end: Option<u64>,
    resume_metadata: Option<&ResumeMetadata>,
) -> crate::Result<(reqwest::Response, String)> {
    send_path_request_with_clients(
        route,
        custom_header,
        credentials,
        download_meta,
        range_start,
        range_end,
        resume_metadata,
        &NO_REDIRECT_REQWEST_CLIENT,
        &DIRECT_REQWEST_CLIENT,
    )
    .await
}

#[derive(Clone, Copy)]
struct ByteRange {
    index: usize,
    start: u64,
    end: u64,
}

struct SegmentedDownloadSuccess {
    size: u64,
    final_url: String,
    ttfb: time::Duration,
    transfer_elapsed: time::Duration,
}

enum SegmentedDownloadOutcome {
    Success(SegmentedDownloadSuccess),
    FallbackSingle { disable_range: bool },
    Fatal(crate::Error),
}

enum SegmentDownloadError {
    Protocol,
    Transport,
    Fatal(crate::Error),
}

fn segmented_ranges(size: u64) -> Vec<ByteRange> {
    let segment_count = if size <= 64 * 1024 * 1024 {
        2
    } else if size <= 192 * 1024 * 1024 {
        3
    } else {
        4
    };
    let segment_count = segment_count
        .min(usize::try_from(size / MIN_SEGMENT_SIZE).unwrap_or(4).max(1));
    let segment_size = size.div_ceil(segment_count as u64);
    (0..segment_count)
        .map(|index| {
            let start = index as u64 * segment_size;
            ByteRange {
                index,
                start,
                end: (start + segment_size - 1).min(size - 1),
            }
        })
        .collect()
}

fn response_resume_metadata(
    route: &DownloadRoute,
    integrity: &Integrity,
    response: &reqwest::Response,
) -> ResumeMetadata {
    ResumeMetadata {
        url: route.url.clone(),
        etag: response
            .headers()
            .get(header::ETAG)
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.starts_with("W/"))
            .map(str::to_string),
        last_modified: response
            .headers()
            .get(header::LAST_MODIFIED)
            .and_then(|value| value.to_str().ok())
            .filter(|value| DateTime::parse_from_rfc2822(value).is_ok())
            .map(str::to_string),
        expected_size: integrity.size,
        sha1: integrity.sha1.clone(),
        sha512: integrity.sha512.clone(),
        sha256: integrity.sha256.clone(),
        md5: integrity.md5.clone(),
    }
}

fn range_validator_matches(
    expected: &ResumeMetadata,
    response: &ResumeMetadata,
) -> bool {
    if let Some(etag) = &expected.etag {
        return response.etag.as_ref() == Some(etag);
    }
    expected
        .last_modified
        .as_ref()
        .is_some_and(|last_modified| {
            response.last_modified.as_ref() == Some(last_modified)
        })
}

fn segment_path(part_path: &Path, index: usize) -> PathBuf {
    suffixed_path(part_path, &format!(".segment-{index}"))
}

async fn cleanup_segment_files(
    part_path: &Path,
    segment_count: usize,
) -> crate::Result<()> {
    for index in 0..segment_count {
        remove_if_exists(&segment_path(part_path, index)).await?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn download_segment(
    route: &DownloadRoute,
    range: ByteRange,
    total_size: u64,
    validator: &ResumeMetadata,
    custom_header: Option<&(String, String)>,
    credentials: Option<&crate::state::ModrinthCredentials>,
    download_meta: Option<&DownloadMeta>,
    part_path: &Path,
    semaphore: &FetchSemaphore,
    system_client: &reqwest::Client,
    direct_client: &reqwest::Client,
    progress: tokio::sync::mpsc::UnboundedSender<u64>,
) -> Result<(), SegmentDownloadError> {
    let permit = semaphore
        .0
        .acquire()
        .await
        .map_err(|error| SegmentDownloadError::Fatal(error.into()))?;
    let (response, _) = send_path_request_with_clients(
        route,
        custom_header,
        credentials,
        download_meta,
        Some(range.start),
        Some(range.end),
        Some(validator),
        system_client,
        direct_client,
    )
    .await
    .map_err(|_| SegmentDownloadError::Transport)?;
    if response.status() != StatusCode::PARTIAL_CONTENT {
        drop(permit);
        return Err(if response.status().is_success() {
            SegmentDownloadError::Protocol
        } else {
            SegmentDownloadError::Transport
        });
    }
    let expected_range = ParsedContentRange {
        start: range.start,
        end: range.end,
        total: total_size,
    };
    if parse_content_range(&response) != Some(expected_range) {
        drop(permit);
        return Err(SegmentDownloadError::Protocol);
    }
    let response_validator =
        response_resume_metadata(route, &Integrity::default(), &response);
    if !range_validator_matches(validator, &response_validator) {
        drop(permit);
        return Err(SegmentDownloadError::Protocol);
    }

    let path = segment_path(part_path, range.index);
    let mut file = File::create(&path).await.map_err(|error| {
        SegmentDownloadError::Fatal(IOError::with_path(error, &path).into())
    })?;
    let mut stream = response.bytes_stream();
    let mut downloaded = 0_u64;
    let mut pending_progress = 0_u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| SegmentDownloadError::Transport)?;
        file.write_all(&chunk).await.map_err(|error| {
            SegmentDownloadError::Fatal(IOError::with_path(error, &path).into())
        })?;
        downloaded += chunk.len() as u64;
        pending_progress += chunk.len() as u64;
        if pending_progress >= 256 * 1024 {
            let _ = progress.send(pending_progress);
            pending_progress = 0;
        }
    }
    if pending_progress > 0 {
        let _ = progress.send(pending_progress);
    }
    file.flush().await.map_err(|error| {
        SegmentDownloadError::Fatal(IOError::with_path(error, &path).into())
    })?;
    file.sync_data().await.map_err(|error| {
        SegmentDownloadError::Fatal(IOError::with_path(error, &path).into())
    })?;
    drop(file);
    drop(permit);
    if downloaded != range.end - range.start + 1 {
        return Err(SegmentDownloadError::Protocol);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn try_segmented_download(
    request: &DownloadRequest,
    route: &DownloadRoute,
    size: u64,
    part_path: &Path,
    semaphore: &FetchSemaphore,
    credentials: Option<&crate::state::ModrinthCredentials>,
    mut progress: Option<&mut FetchProgressFn<'_>>,
    system_client: &reqwest::Client,
    direct_client: &reqwest::Client,
) -> SegmentedDownloadOutcome {
    let ranges = segmented_ranges(size);
    if ranges.len() < 2 {
        return SegmentedDownloadOutcome::FallbackSingle {
            disable_range: false,
        };
    }
    if let Err(error) = cleanup_segment_files(part_path, ranges.len()).await {
        return SegmentedDownloadOutcome::Fatal(error);
    }

    let permit = match semaphore.0.acquire().await {
        Ok(permit) => permit,
        Err(error) => return SegmentedDownloadOutcome::Fatal(error.into()),
    };
    let probe_started = Instant::now();
    let (probe, final_url) = match send_path_request_with_clients(
        route,
        request.header.as_ref(),
        credentials,
        None,
        Some(0),
        Some(0),
        None,
        system_client,
        direct_client,
    )
    .await
    {
        Ok(response) => response,
        Err(_) => {
            drop(permit);
            return SegmentedDownloadOutcome::FallbackSingle {
                disable_range: false,
            };
        }
    };
    let ttfb = probe_started.elapsed();
    if probe.status() != StatusCode::PARTIAL_CONTENT
        || parse_content_range(&probe)
            != Some(ParsedContentRange {
                start: 0,
                end: 0,
                total: size,
            })
    {
        let disable_range = probe.status().is_success();
        drop(probe);
        drop(permit);
        return SegmentedDownloadOutcome::FallbackSingle { disable_range };
    }
    let validator = response_resume_metadata(route, &request.integrity, &probe);
    if validator.etag.is_none() && validator.last_modified.is_none() {
        drop(probe);
        drop(permit);
        return SegmentedDownloadOutcome::FallbackSingle {
            disable_range: false,
        };
    }
    let probe_body = match probe.bytes().await {
        Ok(bytes) => bytes,
        Err(_) => {
            drop(permit);
            return SegmentedDownloadOutcome::FallbackSingle {
                disable_range: false,
            };
        }
    };
    drop(permit);
    if probe_body.len() != 1 {
        return SegmentedDownloadOutcome::FallbackSingle {
            disable_range: true,
        };
    }

    let transfer_started = Instant::now();
    let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut downloads = futures::stream::FuturesUnordered::new();
    for range in ranges.iter().copied() {
        downloads.push(download_segment(
            route,
            range,
            size,
            &validator,
            request.header.as_ref(),
            credentials,
            (range.index == 0)
                .then_some(request.download_meta.as_ref())
                .flatten(),
            part_path,
            semaphore,
            system_client,
            direct_client,
            progress_tx.clone(),
        ));
    }
    drop(progress_tx);
    let mut downloaded = 0_u64;
    let mut segment_error = None;
    while !downloads.is_empty() {
        tokio::select! {
            Some(delta) = progress_rx.recv() => {
                downloaded = downloaded.saturating_add(delta);
                if let Some(progress) = progress.as_mut()
                    && let Err(error) = progress(downloaded, size).await
                {
                    segment_error = Some(SegmentDownloadError::Fatal(error));
                    break;
                }
            }
            result = downloads.next() => {
                if let Some(Err(error)) = result {
                    segment_error = Some(error);
                    break;
                }
            }
        }
    }
    drop(downloads);
    while let Ok(delta) = progress_rx.try_recv() {
        downloaded = downloaded.saturating_add(delta);
    }
    if let Some(error) = segment_error {
        let _ = cleanup_segment_files(part_path, ranges.len()).await;
        return match error {
            SegmentDownloadError::Protocol => {
                SegmentedDownloadOutcome::FallbackSingle {
                    disable_range: true,
                }
            }
            SegmentDownloadError::Transport => {
                SegmentedDownloadOutcome::FallbackSingle {
                    disable_range: false,
                }
            }
            SegmentDownloadError::Fatal(error) => {
                SegmentedDownloadOutcome::Fatal(error)
            }
        };
    }

    let mut output = match File::create(part_path).await {
        Ok(file) => file,
        Err(error) => {
            return SegmentedDownloadOutcome::Fatal(
                IOError::with_path(error, part_path).into(),
            );
        }
    };
    let mut hashers = IntegrityHashers::new(&request.integrity);
    let mut merged_size = 0_u64;
    let mut buffer = vec![0_u8; 256 * 1024];
    for range in &ranges {
        let path = segment_path(part_path, range.index);
        let mut segment = match File::open(&path).await {
            Ok(file) => file,
            Err(error) => {
                return SegmentedDownloadOutcome::Fatal(
                    IOError::with_path(error, &path).into(),
                );
            }
        };
        loop {
            let read = match segment.read(&mut buffer).await {
                Ok(read) => read,
                Err(error) => {
                    return SegmentedDownloadOutcome::Fatal(
                        IOError::with_path(error, &path).into(),
                    );
                }
            };
            if read == 0 {
                break;
            }
            if let Err(error) = output.write_all(&buffer[..read]).await {
                return SegmentedDownloadOutcome::Fatal(
                    IOError::with_path(error, part_path).into(),
                );
            }
            hashers.update(&buffer[..read]);
            merged_size += read as u64;
        }
        if let Err(error) = remove_if_exists(&path).await {
            return SegmentedDownloadOutcome::Fatal(error);
        }
    }
    if let Err(error) = output.flush().await {
        return SegmentedDownloadOutcome::Fatal(
            IOError::with_path(error, part_path).into(),
        );
    }
    if let Err(error) = output.sync_data().await {
        return SegmentedDownloadOutcome::Fatal(
            IOError::with_path(error, part_path).into(),
        );
    }
    drop(output);
    if merged_size != size {
        let _ = remove_if_exists(part_path).await;
        return SegmentedDownloadOutcome::FallbackSingle {
            disable_range: true,
        };
    }
    let computed = hashers.finish(merged_size);
    if verify_computed_integrity(&request.integrity, &computed).is_err() {
        let _ = remove_if_exists(part_path).await;
        return SegmentedDownloadOutcome::FallbackSingle {
            disable_range: true,
        };
    }
    if validate_file_content(part_path, request.integrity.content)
        .await
        .is_err()
    {
        let _ = remove_if_exists(part_path).await;
        return SegmentedDownloadOutcome::FallbackSingle {
            disable_range: true,
        };
    }
    if downloaded < size
        && let Some(progress) = progress.as_mut()
        && let Err(error) = progress(size, size).await
    {
        return SegmentedDownloadOutcome::Fatal(error);
    }
    SegmentedDownloadOutcome::Success(SegmentedDownloadSuccess {
        size: merged_size,
        final_url,
        ttfb,
        transfer_elapsed: transfer_started.elapsed(),
    })
}

/// Streams a download to a sibling `.part` file, verifies it, then atomically
/// moves it into place. Safe resumes require a matching URL and ETag or
/// Last-Modified validator stored in the `.part.json` sidecar.
#[tracing::instrument(skip(semaphore, _exec, progress, request, destination))]
pub async fn download_to_path(
    request: DownloadRequest,
    destination: impl AsRef<Path>,
    semaphore: &FetchSemaphore,
    _exec: impl sqlx::Executor<'_, Database = sqlx::Sqlite>,
    mut progress: Option<&mut FetchProgressFn<'_>>,
) -> crate::Result<DownloadResult> {
    let destination = destination.as_ref();
    if let Some(parent) = destination.parent() {
        io::create_dir_all(parent).await?;
    }
    let lock_key = download_lock_key(destination, &request.integrity);
    let download_lock = in_flight_download_lock(lock_key);
    let _download_guard = download_lock.lock().await;
    let allow_cas_hard_links = matches!(
        request.resource,
        ResourceClass::Metadata
            | ResourceClass::MinecraftAsset
            | ResourceClass::MinecraftLibrary
            | ResourceClass::Loader
            | ResourceClass::Java
    );

    let mode = request
        .source_mode
        .unwrap_or_else(|| source_mode_for_resource(request.resource));
    let mut routes = request.routes.clone().unwrap_or_else(|| {
        let mut urls = Vec::with_capacity(request.candidate_urls.len() + 1);
        urls.push(request.url.clone());
        urls.extend(request.candidate_urls.iter().cloned());
        let mut routes = Vec::new();
        for (index, url) in urls.into_iter().enumerate() {
            let mut candidates =
                resolve_download_routes_for(&url, request.resource, mode);
            if index > 0 {
                for candidate in &mut candidates {
                    if !candidate.is_mirror {
                        candidate.source = DownloadRouteSource::Alternate;
                        candidate.allow_sensitive_headers = false;
                    }
                }
            }
            for candidate in candidates {
                if !routes.iter().any(|existing: &DownloadRoute| {
                    existing.url == candidate.url
                        && existing.proxy == candidate.proxy
                }) {
                    routes.push(candidate);
                }
            }
        }
        routes
    });
    if !request.integrity.has_verified_content_hash() {
        routes
            .retain(|route| route.source != DownloadRouteSource::TencentMaven);
    }
    let credentials: Option<crate::state::ModrinthCredentials> = None;
    if request
        .header
        .as_ref()
        .is_some_and(|(name, _)| header_requires_official_only(name))
    {
        routes.retain(|route| !route.is_mirror);
    }
    if routes.is_empty() {
        routes.push(official_route(&request.url, request.resource));
    }
    let part_path = suffixed_path(destination, ".part");
    let metadata_path = suffixed_path(destination, ".part.json");

    if !request.integrity.is_empty()
        && tokio::fs::try_exists(destination)
            .await
            .map_err(|error| IOError::with_path(error, destination))?
        && let Ok(size) = verify_file(destination, &request.integrity).await
    {
        let route = routes
            .first()
            .cloned()
            .unwrap_or_else(|| official_route(&request.url, request.resource));
        remove_if_exists(&part_path).await?;
        remove_if_exists(&metadata_path).await?;
        return Ok(DownloadResult {
            path: destination.to_path_buf(),
            url: route.url,
            source: route.source,
            size,
            resumed_bytes: 0,
            attempts: 0,
            fallback_count: 0,
        });
    }
    if let Some(cache_path) = cas_path(&request.integrity)
        && tokio::fs::try_exists(&cache_path)
            .await
            .map_err(|error| IOError::with_path(error, &cache_path))?
    {
        match verify_file(&cache_path, &request.integrity).await {
            Ok(size) => {
                materialize_cached_download(
                    &cache_path,
                    destination,
                    allow_cas_hard_links,
                )
                .await?;
                remove_if_exists(&part_path).await?;
                remove_if_exists(&metadata_path).await?;
                let route = routes.first().cloned().unwrap_or_else(|| {
                    official_route(&request.url, request.resource)
                });
                return Ok(DownloadResult {
                    path: destination.to_path_buf(),
                    url: route.url,
                    source: route.source,
                    size,
                    resumed_bytes: 0,
                    attempts: 0,
                    fallback_count: 0,
                });
            }
            Err(error) => {
                tracing::warn!(
                    path = %cache_path.display(),
                    error = %error,
                    "Discarding a corrupted download cache entry"
                );
                remove_if_exists(&cache_path).await?;
            }
        }
    }

    if request.resume == ResumePolicy::Disabled {
        remove_if_exists(&part_path).await?;
        remove_if_exists(&metadata_path).await?;
    }

    if request.resume == ResumePolicy::Safe
        && let Some(metadata) = read_resume_metadata(&metadata_path).await
        && let Some(route) = routes.iter().find(|route| {
            resume_metadata_matches(&metadata, route, &request.integrity)
        })
        && tokio::fs::try_exists(&part_path)
            .await
            .map_err(|error| IOError::with_path(error, &part_path))?
        && let Ok(size) = verify_file(&part_path, &request.integrity).await
    {
        finalize_download(&part_path, &metadata_path, destination).await?;
        if let Err(error) = cache_completed_download(
            destination,
            &request.integrity,
            allow_cas_hard_links,
        )
        .await
        {
            tracing::warn!(
                path = %destination.display(),
                error = %error,
                "Unable to populate the shared download cache"
            );
        }
        return Ok(DownloadResult {
            path: destination.to_path_buf(),
            url: route.url.clone(),
            source: route.source,
            size,
            resumed_bytes: size,
            attempts: 0,
            fallback_count: 0,
        });
    }

    let mut attempts = 0;
    let mut last_error = None;
    let mut fallback_count = 0;
    for (route_index, route) in routes.iter().enumerate() {
        if route_index > 0 {
            fallback_count += 1;
            let can_keep_partial = read_resume_metadata(&metadata_path)
                .await
                .is_some_and(|metadata| {
                    resume_metadata_matches(
                        &metadata,
                        route,
                        &request.integrity,
                    )
                });
            if !can_keep_partial {
                remove_if_exists(&part_path).await?;
                remove_if_exists(&metadata_path).await?;
            }
        }
        let mut retried_server_error = false;
        let mut retried_transfer = false;
        let mut segmented_attempted = false;
        while attempts < ARTIFACT_ATTEMPT_BUDGET {
            let remaining_routes = routes.len() - route_index - 1;
            attempts += 1;
            let existing_metadata = if request.resume == ResumePolicy::Safe {
                read_resume_metadata(&metadata_path).await
            } else {
                None
            };
            let part_size = tokio::fs::metadata(&part_path)
                .await
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            let resume_from = existing_metadata
                .as_ref()
                .filter(|metadata| {
                    route.supports_range
                        && request
                            .integrity
                            .size
                            .is_none_or(|expected| part_size < expected)
                        && resume_metadata_matches(
                            metadata,
                            route,
                            &request.integrity,
                        )
                })
                .map_or(0, |_| part_size);

            if !segmented_attempted
                && resume_from == 0
                && route.supports_range
                && range_splitting_allowed(route)
                && request
                    .integrity
                    .size
                    .is_some_and(|size| size > SEGMENTED_DOWNLOAD_THRESHOLD)
            {
                segmented_attempted = true;
                let size = request.integrity.size.unwrap();
                match try_segmented_download(
                    &request,
                    route,
                    size,
                    &part_path,
                    semaphore,
                    credentials.as_ref(),
                    progress.as_deref_mut(),
                    &NO_REDIRECT_REQWEST_CLIENT,
                    &DIRECT_REQWEST_CLIENT,
                )
                .await
                {
                    SegmentedDownloadOutcome::Success(result) => {
                        finalize_download(
                            &part_path,
                            &metadata_path,
                            destination,
                        )
                        .await?;
                        if let Err(error) = cache_completed_download(
                            destination,
                            &request.integrity,
                            allow_cas_hard_links,
                        )
                        .await
                        {
                            tracing::warn!(
                                path = %destination.display(),
                                error = %error,
                                "Unable to populate the shared download cache"
                            );
                        }
                        record_route_success(
                            route,
                            result.ttfb,
                            result.size,
                            result.transfer_elapsed,
                        );
                        return Ok(DownloadResult {
                            path: destination.to_path_buf(),
                            url: result.final_url,
                            source: route.source,
                            size: result.size,
                            resumed_bytes: 0,
                            attempts,
                            fallback_count,
                        });
                    }
                    SegmentedDownloadOutcome::FallbackSingle {
                        disable_range,
                    } => {
                        if disable_range {
                            disable_range_splitting(route);
                        }
                        remove_if_exists(&part_path).await?;
                        remove_if_exists(&metadata_path).await?;
                    }
                    SegmentedDownloadOutcome::Fatal(error) => {
                        return Err(error);
                    }
                }
            }

            let permit = semaphore.0.acquire().await?;
            let request_started = Instant::now();
            let (response, final_url) = match send_path_request(
                route,
                request.header.as_ref(),
                credentials.as_ref(),
                request.download_meta.as_ref(),
                (resume_from > 0).then_some(resume_from),
                None,
                existing_metadata.as_ref(),
            )
            .await
            {
                Ok(response) => response,
                Err(error) => {
                    drop(permit);
                    record_route_failure(route);
                    last_error = Some(error);
                    if route_index + 1 < routes.len() {
                        break;
                    }
                    if attempts < ARTIFACT_ATTEMPT_BUDGET {
                        tokio::time::sleep(fetch_retry_delay(attempts)).await;
                        continue;
                    }
                    break;
                }
            };
            let ttfb = request_started.elapsed();
            let status = response.status();
            let response_retry_after = retry_after(&response);
            if status.is_client_error() || status.is_server_error() {
                record_route_failure(route);
                let error =
                    response_status_error(response, &Method::GET, &route.url)
                        .await;
                drop(permit);
                last_error = Some(error);
                if status == StatusCode::RANGE_NOT_SATISFIABLE
                    && resume_from > 0
                    && attempts + remaining_routes < ARTIFACT_ATTEMPT_BUDGET
                {
                    remove_if_exists(&part_path).await?;
                    remove_if_exists(&metadata_path).await?;
                    continue;
                }
                if status == StatusCode::TOO_MANY_REQUESTS
                    && route_index + 1 == routes.len()
                    && attempts < ARTIFACT_ATTEMPT_BUDGET
                {
                    tokio::time::sleep(
                        response_retry_after
                            .unwrap_or_else(|| fetch_retry_delay(attempts)),
                    )
                    .await;
                    continue;
                }
                if status.is_server_error()
                    && !retried_server_error
                    && attempts + remaining_routes < ARTIFACT_ATTEMPT_BUDGET
                {
                    retried_server_error = true;
                    tokio::time::sleep(fetch_retry_delay(attempts)).await;
                    continue;
                }
                break;
            }

            let resumed = resume_from > 0
                && status == StatusCode::PARTIAL_CONTENT
                && content_range_starts_at(&response, resume_from);
            if resume_from > 0
                && status == StatusCode::PARTIAL_CONTENT
                && !resumed
            {
                drop(response);
                drop(permit);
                record_route_failure(route);
                remove_if_exists(&part_path).await?;
                remove_if_exists(&metadata_path).await?;
                last_error = Some(
                    ErrorKind::OtherError(format!(
                        "Invalid Content-Range response from {}",
                        route.url
                    ))
                    .into(),
                );
                if route_index + 1 == routes.len()
                    && attempts < ARTIFACT_ATTEMPT_BUDGET
                {
                    continue;
                }
                break;
            }
            if resume_from > 0 && !resumed {
                remove_if_exists(&part_path).await?;
                remove_if_exists(&metadata_path).await?;
            }
            let starting_size = if resumed { resume_from } else { 0 };
            let mut hashers = IntegrityHashers::new(&request.integrity);
            if starting_size > 0 {
                let mut partial = File::open(&part_path)
                    .await
                    .map_err(|error| IOError::with_path(error, &part_path))?;
                let mut buffer = vec![0_u8; 256 * 1024];
                loop {
                    let read =
                        partial.read(&mut buffer).await.map_err(|error| {
                            IOError::with_path(error, &part_path)
                        })?;
                    if read == 0 {
                        break;
                    }
                    hashers.update(&buffer[..read]);
                }
            }

            let response_metadata = ResumeMetadata {
                url: route.url.clone(),
                etag: response
                    .headers()
                    .get(header::ETAG)
                    .and_then(|value| value.to_str().ok())
                    .filter(|value| !value.starts_with("W/"))
                    .map(str::to_string),
                last_modified: response
                    .headers()
                    .get(header::LAST_MODIFIED)
                    .and_then(|value| value.to_str().ok())
                    .filter(|value| DateTime::parse_from_rfc2822(value).is_ok())
                    .map(str::to_string),
                expected_size: request.integrity.size,
                sha1: request.integrity.sha1.clone(),
                sha512: request.integrity.sha512.clone(),
                sha256: request.integrity.sha256.clone(),
                md5: request.integrity.md5.clone(),
            };
            write_resume_metadata(&metadata_path, &response_metadata).await?;

            let mut file = if resumed {
                OpenOptions::new()
                    .append(true)
                    .open(&part_path)
                    .await
                    .map_err(|error| IOError::with_path(error, &part_path))?
            } else {
                File::create(&part_path)
                    .await
                    .map_err(|error| IOError::with_path(error, &part_path))?
            };
            let response_length = response.content_length().unwrap_or(0);
            let total_size = request
                .integrity
                .size
                .unwrap_or(starting_size.saturating_add(response_length));
            let transfer_started = Instant::now();
            let mut downloaded = starting_size;
            let mut stream = response.bytes_stream();
            let mut transfer_error = None;
            while let Some(item) = stream.next().await {
                let chunk = match item {
                    Ok(chunk) => chunk,
                    Err(error) => {
                        transfer_error = Some(error);
                        break;
                    }
                };
                file.write_all(&chunk)
                    .await
                    .map_err(|error| IOError::with_path(error, &part_path))?;
                hashers.update(&chunk);
                downloaded += chunk.len() as u64;
                if let Some(progress) = progress.as_mut() {
                    progress(downloaded, total_size).await?;
                }
            }
            file.flush()
                .await
                .map_err(|error| IOError::with_path(error, &part_path))?;
            file.sync_data()
                .await
                .map_err(|error| IOError::with_path(error, &part_path))?;
            drop(file);
            drop(permit);

            if let Some(error) = transfer_error {
                record_route_failure(route);
                last_error = Some(error.into());
                if !retried_transfer
                    && attempts + remaining_routes < ARTIFACT_ATTEMPT_BUDGET
                {
                    retried_transfer = true;
                    tokio::time::sleep(fetch_retry_delay(attempts)).await;
                    continue;
                }
                break;
            }

            let computed = hashers.finish(downloaded);
            if let Err(error) =
                verify_computed_integrity(&request.integrity, &computed)
            {
                record_route_failure(route);
                remove_if_exists(&part_path).await?;
                remove_if_exists(&metadata_path).await?;
                last_error = Some(error);
                if route_index + 1 == routes.len()
                    && attempts < ARTIFACT_ATTEMPT_BUDGET
                {
                    tokio::time::sleep(fetch_retry_delay(attempts)).await;
                    continue;
                }
                break;
            }
            if let Err(error) =
                validate_file_content(&part_path, request.integrity.content)
                    .await
            {
                record_route_failure(route);
                remove_if_exists(&part_path).await?;
                remove_if_exists(&metadata_path).await?;
                last_error = Some(error);
                break;
            }

            finalize_download(&part_path, &metadata_path, destination).await?;
            if let Err(error) = cache_completed_download(
                destination,
                &request.integrity,
                allow_cas_hard_links,
            )
            .await
            {
                tracing::warn!(
                    path = %destination.display(),
                    error = %error,
                    "Unable to populate the shared download cache"
                );
            }
            record_route_success(
                route,
                ttfb,
                downloaded.saturating_sub(starting_size),
                transfer_started.elapsed(),
            );
            return Ok(DownloadResult {
                path: destination.to_path_buf(),
                url: final_url,
                source: route.source,
                size: downloaded,
                resumed_bytes: starting_size,
                attempts,
                fallback_count,
            });
        }
    }

    Err(last_error.unwrap_or_else(|| {
        ErrorKind::OtherError(format!(
            "Unable to download {} from any source",
            request.url
        ))
        .into()
    }))
}

/// Downloads a file from specified mirrors
#[tracing::instrument(skip(semaphore))]
pub async fn fetch_mirrors(
    mirrors: &[&str],
    sha1: Option<&str>,
    download_meta: Option<&DownloadMeta>,
    uri_path: Option<&'static str>,
    semaphore: &FetchSemaphore,
    exec: impl sqlx::Executor<'_, Database = sqlx::Sqlite> + Copy,
) -> crate::Result<Bytes> {
    if mirrors.is_empty() {
        return Err(
            ErrorKind::InputError("No mirrors provided!".to_string()).into()
        );
    }

    let route_count = mirrors.len().min(ARTIFACT_ATTEMPT_BUDGET);
    let mut last_error = None;
    for (index, mirror) in mirrors.iter().take(route_count).enumerate() {
        let attempt_budget = if index + 1 == route_count {
            ARTIFACT_ATTEMPT_BUDGET - index
        } else {
            1
        };
        match fetch_advanced_with_client_and_progress(
            Method::GET,
            mirror,
            sha1,
            None,
            None,
            download_meta,
            None,
            uri_path,
            semaphore,
            exec,
            &REQWEST_CLIENT,
            None,
            None,
            None,
            attempt_budget,
        )
        .await
        {
            Ok(bytes) => return Ok(bytes),
            Err(error) => last_error = Some(error),
        }
    }

    Err(last_error.unwrap_or_else(|| {
        ErrorKind::OtherError("Unable to download from any mirror".to_string())
            .into()
    }))
}

#[tracing::instrument(skip(semaphore, progress))]
pub async fn fetch_mirrors_with_progress(
    mirrors: &[&str],
    sha1: Option<&str>,
    download_meta: Option<&DownloadMeta>,
    uri_path: Option<&'static str>,
    semaphore: &FetchSemaphore,
    exec: impl sqlx::Executor<'_, Database = sqlx::Sqlite> + Copy,
    mut progress: Option<&mut FetchProgressFn<'_>>,
) -> crate::Result<Bytes> {
    if mirrors.is_empty() {
        return Err(
            ErrorKind::InputError("No mirrors provided!".to_string()).into()
        );
    }

    let route_count = mirrors.len().min(ARTIFACT_ATTEMPT_BUDGET);
    let mut last_error = None;
    for (index, mirror) in mirrors.iter().take(route_count).enumerate() {
        let attempt_budget = if index + 1 == route_count {
            ARTIFACT_ATTEMPT_BUDGET - index
        } else {
            1
        };
        match fetch_advanced_with_client_and_progress(
            Method::GET,
            mirror,
            sha1,
            None,
            None,
            download_meta,
            None,
            uri_path,
            semaphore,
            exec,
            &REQWEST_CLIENT,
            progress.as_deref_mut(),
            None,
            None,
            attempt_budget,
        )
        .await
        {
            Ok(bytes) => return Ok(bytes),
            Err(error) => last_error = Some(error),
        }
    }

    Err(last_error.unwrap_or_else(|| {
        ErrorKind::OtherError("Unable to download from any mirror".to_string())
            .into()
    }))
}

#[tracing::instrument(skip(semaphore, progress, attempt_reporter))]
pub async fn fetch_mirrors_with_progress_and_attempts(
    mirrors: &[&str],
    sha1: Option<&str>,
    download_meta: Option<&DownloadMeta>,
    uri_path: Option<&'static str>,
    semaphore: &FetchSemaphore,
    exec: impl sqlx::Executor<'_, Database = sqlx::Sqlite> + Copy,
    mut progress: Option<&mut FetchProgressFn<'_>>,
    mut attempt_reporter: Option<&mut FetchAttemptFn<'_>>,
) -> crate::Result<Bytes> {
    if mirrors.is_empty() {
        return Err(
            ErrorKind::InputError("No mirrors provided!".to_string()).into()
        );
    }

    let route_count = mirrors.len().min(ARTIFACT_ATTEMPT_BUDGET);
    let mut last_error = None;
    for (index, mirror) in mirrors.iter().take(route_count).enumerate() {
        let attempt_budget = if index + 1 == route_count {
            ARTIFACT_ATTEMPT_BUDGET - index
        } else {
            1
        };
        match fetch_advanced_with_client_and_progress(
            Method::GET,
            mirror,
            sha1,
            None,
            None,
            download_meta,
            None,
            uri_path,
            semaphore,
            exec,
            &REQWEST_CLIENT,
            progress.as_deref_mut(),
            attempt_reporter.as_deref_mut(),
            None,
            attempt_budget,
        )
        .await
        {
            Ok(bytes) => return Ok(bytes),
            Err(error) => last_error = Some(error),
        }
    }

    Err(last_error.unwrap_or_else(|| {
        ErrorKind::OtherError("Unable to download from any mirror".to_string())
            .into()
    }))
}

/// Posts a JSON to a URL
#[tracing::instrument(skip(json_body, semaphore))]
pub async fn post_json(
    url: &str,
    json_body: serde_json::Value,
    semaphore: &FetchSemaphore,
    exec: impl sqlx::Executor<'_, Database = sqlx::Sqlite>,
) -> crate::Result<()> {
    let _permit = semaphore.0.acquire().await?;

    let mut req = INSECURE_REQWEST_CLIENT.post(url).json(&json_body);

    if let Some(creds) =
        crate::state::ModrinthCredentials::get_active(exec).await?
    {
        req = req.header("Authorization", &creds.session);
    }

    req.send().await?.error_for_status()?;
    Ok(())
}

pub async fn read_json<T>(
    path: &Path,
    semaphore: &IoSemaphore,
) -> crate::Result<T>
where
    T: DeserializeOwned,
{
    let _permit = semaphore.0.acquire().await?;

    let json = io::read(path).await?;
    let json = serde_json::from_slice::<T>(&json)?;

    Ok(json)
}

#[tracing::instrument(skip(bytes, semaphore))]
pub async fn write(
    path: &Path,
    bytes: &[u8],
    semaphore: &IoSemaphore,
) -> crate::Result<()> {
    let _permit = semaphore.0.acquire().await?;

    if let Some(parent) = path.parent() {
        io::create_dir_all(parent).await?;
    }

    let mut file = File::create(path)
        .await
        .map_err(|e| IOError::with_path(e, path))?;
    file.write_all(bytes)
        .await
        .map_err(|e| IOError::with_path(e, path))?;
    tracing::trace!("Done writing file {}", path.display());
    Ok(())
}

pub async fn copy(
    src: impl AsRef<Path>,
    dest: impl AsRef<Path>,
    semaphore: &IoSemaphore,
) -> crate::Result<()> {
    let src: &Path = src.as_ref();
    let dest = dest.as_ref();

    let _permit = semaphore.0.acquire().await?;

    if let Some(parent) = dest.parent() {
        io::create_dir_all(parent).await?;
    }

    io::copy(src, dest).await?;
    tracing::trace!(
        "Done copying file {} to {}",
        src.display(),
        dest.display()
    );
    Ok(())
}

// Writes a icon to the cache and returns the absolute path of the icon within the cache directory
#[tracing::instrument(skip(bytes, semaphore))]
pub async fn write_cached_icon(
    icon_path: &str,
    cache_dir: &Path,
    bytes: Bytes,
    semaphore: &IoSemaphore,
) -> crate::Result<PathBuf> {
    let extension = Path::new(&icon_path).extension().and_then(OsStr::to_str);
    let hash = sha1_async(bytes.clone()).await?;
    let path = cache_dir.join("icons").join(if let Some(ext) = extension {
        format!("{hash}.{ext}")
    } else {
        hash
    });

    write(&path, &bytes, semaphore).await?;

    let path = io::canonicalize(path)?;
    Ok(path)
}

pub async fn sha1_async(bytes: Bytes) -> crate::Result<String> {
    let hash = tokio::task::spawn_blocking(move || {
        sha1_smol::Sha1::from(bytes).hexdigest()
    })
    .await?;

    Ok(hash)
}

pub async fn sha1_file_async(
    path: impl AsRef<Path>,
) -> crate::Result<(u64, String)> {
    let path = path.as_ref();
    // Local files can be multi-gigabyte .mrpacks, so hash them without materializing bytes.
    let mut file = File::open(path)
        .await
        .map_err(|e| IOError::with_path(e, path))?;
    let mut hasher = sha1_smol::Sha1::new();
    let mut size = 0;
    let mut buffer = vec![0; 262144];

    loop {
        let bytes_read = file
            .read(&mut buffer)
            .await
            .map_err(|e| IOError::with_path(e, path))?;
        if bytes_read == 0 {
            break;
        }

        hasher.update(&buffer[..bytes_read]);
        size += bytes_read as u64;
    }

    Ok((size, hasher.digest().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeDelta, Utc};
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    static RANGE_SPLITTING_TEST_LOCK: LazyLock<AsyncMutex<()>> =
        LazyLock::new(|| AsyncMutex::new(()));

    async fn spawn_range_server(
        data: Arc<Vec<u8>>,
        wrong_content_range: bool,
    ) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
        let listener =
            tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let request_count = requests.clone();
        let handle = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let data = data.clone();
                let requests = request_count.clone();
                tokio::spawn(async move {
                    requests.fetch_add(1, Ordering::Relaxed);
                    let mut request = Vec::new();
                    let mut buffer = [0_u8; 1024];
                    loop {
                        let Ok(read) = stream.read(&mut buffer).await else {
                            return;
                        };
                        if read == 0 {
                            return;
                        }
                        request.extend_from_slice(&buffer[..read]);
                        if request
                            .windows(4)
                            .any(|window| window == b"\r\n\r\n")
                        {
                            break;
                        }
                    }
                    let request =
                        String::from_utf8_lossy(&request).to_ascii_lowercase();
                    let Some(range) = request
                        .lines()
                        .find_map(|line| line.strip_prefix("range: bytes="))
                    else {
                        return;
                    };
                    let Some((start, end)) = range.split_once('-') else {
                        return;
                    };
                    let Ok(start) = start.parse::<u64>() else {
                        return;
                    };
                    let Ok(end) = end.parse::<u64>() else {
                        return;
                    };
                    let body = &data[start as usize..=end as usize];
                    let reported_start = if wrong_content_range {
                        start.saturating_add(1)
                    } else {
                        start
                    };
                    let headers = format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {reported_start}-{end}/{}\r\nETag: \"fixture\"\r\nConnection: close\r\n\r\n",
                        body.len(),
                        data.len(),
                    );
                    if stream.write_all(headers.as_bytes()).await.is_err() {
                        return;
                    }
                    for chunk in body.chunks(64 * 1024) {
                        if stream.write_all(chunk).await.is_err() {
                            return;
                        }
                    }
                });
            }
        });
        (format!("http://{address}/file"), requests, handle)
    }

    fn mirrors(
        minecraft: bool,
        modrinth: bool,
        curseforge: bool,
    ) -> DownloadMirrorSettings {
        DownloadMirrorSettings {
            minecraft,
            modrinth,
            curseforge,
        }
    }

    #[test]
    fn minecraft_mirror_rewrites_supported_urls() {
        let mirrors = mirrors(true, false, false);
        let cases = [
            (
                "https://piston-meta.mojang.com/v1/packages/version.json",
                "https://bmclapi2.bangbang93.com/v1/packages/version.json",
            ),
            (
                "https://resources.download.minecraft.net/ab/abcdef",
                "https://bmclapi2.bangbang93.com/assets/ab/abcdef",
            ),
            (
                "https://libraries.minecraft.net/com/example/library.jar",
                "https://bmclapi2.bangbang93.com/maven/com/example/library.jar",
            ),
            (
                "https://maven.minecraftforge.net/net/minecraftforge/forge.jar",
                "https://bmclapi2.bangbang93.com/maven/net/minecraftforge/forge.jar",
            ),
            (
                "https://maven.fabricmc.net/net/fabricmc/loader.jar",
                "https://bmclapi2.bangbang93.com/maven/net/fabricmc/loader.jar",
            ),
            (
                "https://maven.neoforged.net/releases/net/neoforged/neoforge.jar",
                "https://bmclapi2.bangbang93.com/maven/net/neoforged/neoforge.jar",
            ),
            (
                "https://meta.fabricmc.net/v2/versions/loader?limit=1",
                "https://bmclapi2.bangbang93.com/fabric-meta/v2/versions/loader?limit=1",
            ),
        ];

        for (source, expected) in cases {
            assert_eq!(resolve_download_url(source, mirrors).url, expected);
        }
    }

    #[test]
    fn provider_mirrors_are_independent() {
        let modrinth = mirrors(false, true, false);
        assert_eq!(
            resolve_download_url(
                "https://api.modrinth.com/v2/project",
                modrinth
            ),
            ResolvedDownloadUrl {
                url: "https://mod.mcimirror.top/modrinth/v2/project"
                    .to_string(),
                is_mirror: true,
            }
        );
        assert_eq!(
            resolve_download_url(
                "https://cdn.modrinth.com/data/project/file.jar",
                modrinth,
            )
            .url,
            "https://mod.mcimirror.top/data/project/file.jar"
        );
        assert_eq!(
            resolve_download_url(
                "https://api.curseforge.com/v1/mods/search",
                modrinth,
            )
            .url,
            "https://api.curseforge.com/v1/mods/search"
        );

        let curseforge = mirrors(false, false, true);
        assert_eq!(
            resolve_download_url(
                "https://api.curseforge.com/v1/mods/search",
                curseforge,
            )
            .url,
            "https://mod.mcimirror.top/curseforge/v1/mods/search"
        );
        assert_eq!(
            resolve_download_url(
                "https://edge.forgecdn.net/files/1/2/file.jar",
                curseforge,
            )
            .url,
            "https://mod.mcimirror.top/files/1/2/file.jar"
        );
        assert_eq!(
            resolve_download_url(
                "https://edge.forgecdn.net/files/1/2/file%20name.jar?download=1",
                curseforge,
            )
            .url,
            "https://mod.mcimirror.top/files/1/2/file%20name.jar?download=1"
        );
        for host in ["media.forgecdn.net", "mediafilez.forgecdn.net"] {
            assert_eq!(
                resolve_download_url(
                    &format!("https://{host}/files/1/2/file.jar"),
                    curseforge,
                )
                .url,
                "https://mod.mcimirror.top/files/1/2/file.jar"
            );
        }
        assert_eq!(
            resolve_download_url(
                "https://api.modrinth.com/v2/project",
                curseforge
            )
            .url,
            "https://api.modrinth.com/v2/project"
        );
    }

    #[test]
    fn mirror_resolution_preserves_unmatched_urls() {
        let all = mirrors(true, true, true);
        for url in [
            "https://example.com/file.jar?source=official",
            "https://launcher-meta.modrinth.com/maven/net/fabricmc/loader.jar",
            "https://api.modrinth.com.evil.example/v2/project",
        ] {
            assert_eq!(
                resolve_download_url(url, all),
                ResolvedDownloadUrl {
                    url: url.to_string(),
                    is_mirror: false,
                }
            );
        }

        let disabled = mirrors(false, false, false);
        assert_eq!(
            resolve_download_url(
                "https://resources.download.minecraft.net/ab/abcdef",
                disabled,
            )
            .url,
            "https://resources.download.minecraft.net/ab/abcdef"
        );
    }

    #[test]
    fn mirror_routes_fall_back_to_official_source() {
        let source = "https://api.modrinth.com/v2/project";
        assert_eq!(
            resolve_download_routes(source, mirrors(false, true, false)),
            vec![
                ResolvedDownloadUrl {
                    url: "https://mod.mcimirror.top/modrinth/v2/project"
                        .to_string(),
                    is_mirror: true,
                },
                ResolvedDownloadUrl {
                    url: source.to_string(),
                    is_mirror: false,
                },
            ]
        );
        assert_eq!(
            resolve_download_routes(source, mirrors(false, false, false)),
            vec![ResolvedDownloadUrl {
                url: source.to_string(),
                is_mirror: false,
            }]
        );
    }

    #[test]
    fn modrinth_requests_are_classified_for_logging() {
        assert_eq!(
            modrinth_request_kind("https://api.modrinth.com/v2/project"),
            Some("API")
        );
        assert_eq!(
            modrinth_request_kind(
                "https://cdn.modrinth.com/data/project/version/file.jar"
            ),
            Some("CDN")
        );
        assert_eq!(modrinth_request_kind("https://example.com/file.jar"), None);
    }

    #[test]
    fn modrinth_cdn_redirects_only_fall_back_to_official_cdn() {
        assert!(is_official_modrinth_cdn_redirect(Some(
            "https://cdn.modrinth.com/data/project/versions/version/file.jar"
        )));
        assert!(is_official_modrinth_cdn_redirect(Some(
            "https://CDN.MODRINTH.COM/data/project/versions/version/file.jar"
        )));
        assert!(!is_official_modrinth_cdn_redirect(Some(
            "https://cache.mcimirror.top/data/project/versions/version/file.jar"
        )));
        assert!(!is_official_modrinth_cdn_redirect(Some(
            "https://cdn.modrinth.com.evil.example/file.jar"
        )));
        assert!(!is_official_modrinth_cdn_redirect(Some(
            "https://cdn.modrinth.com@evil.example/file.jar"
        )));
        assert!(!is_official_modrinth_cdn_redirect(Some(
            "https://cdn.modrinth.com/\u{4e0b}\u{8f7d}/file.jar"
        )));
        assert!(!is_official_modrinth_cdn_redirect(None));
    }

    #[test]
    fn redirect_locations_are_bounded_ascii_values() {
        assert!(is_safe_redirect_location("/download/file.jar"));
        assert!(!is_safe_redirect_location("/\u{4e0b}\u{8f7d}/file.jar"));
        assert!(!is_safe_redirect_location(
            &"a".repeat(MAX_REDIRECT_LOCATION_BYTES + 1)
        ));
    }

    #[test]
    fn mrpack_urls_are_detected_without_query_string() {
        assert!(is_mrpack_url(
            "https://cdn.modrinth.com/data/project/version/pack.MRPACK?download=1"
        ));
        assert!(!is_mrpack_url(
            "https://cdn.modrinth.com/data/project/version/mod.jar"
        ));
    }

    #[test]
    fn fetch_retries_use_short_jittered_backoff() {
        let cases = [
            (1, Duration::from_millis(212), Duration::from_millis(288)),
            (2, Duration::from_millis(637), Duration::from_millis(863)),
            (3, Duration::from_millis(1700), Duration::from_millis(2300)),
            (4, Duration::from_millis(1700), Duration::from_millis(2300)),
        ];
        for (attempt, minimum, maximum) in cases {
            let delay = fetch_retry_delay(attempt);
            assert!(delay >= minimum, "attempt {attempt}: {delay:?}");
            assert!(delay <= maximum, "attempt {attempt}: {delay:?}");
        }
    }

    #[test]
    fn vanilla_libraries_have_both_bmcl_routes() {
        let source = "https://libraries.minecraft.net/com/example/library/1/library-1.jar";
        let routes = resolve_download_routes_for(
            source,
            ResourceClass::MinecraftLibrary,
            crate::state::DownloadSourceMode::MirrorPreferred,
        );
        assert_eq!(routes.len(), 3);
        assert_eq!(
            routes[0].url,
            "https://bmclapi2.bangbang93.com/maven/com/example/library/1/library-1.jar"
        );
        assert_eq!(
            routes[1].url,
            "https://bmclapi2.bangbang93.com/libraries/com/example/library/1/library-1.jar"
        );
        assert_eq!(routes[2].url, source);
    }

    #[test]
    fn tencent_maven_routes_are_limited_to_maven_central() {
        for source in [
            "https://repo1.maven.org/maven2/com/example/library/1/library-1.jar?download=1",
            "https://repo.maven.apache.org/maven2/com/example/library/1/library-1.jar?download=1",
        ] {
            let routes = resolve_download_routes_for(
                source,
                ResourceClass::MinecraftLibrary,
                crate::state::DownloadSourceMode::MirrorPreferred,
            );
            assert_eq!(routes.len(), 2);
            assert_eq!(
                routes[0].url,
                "https://mirrors.cloud.tencent.com/nexus/repository/maven-public/com/example/library/1/library-1.jar?download=1"
            );
            assert_eq!(routes[0].source, DownloadRouteSource::TencentMaven);
            assert!(routes[0].is_mirror);
            assert!(!routes[0].allow_sensitive_headers);
            assert_eq!(routes[1].url, source);
        }

        let unmatched = resolve_download_routes_for(
            "https://repo1.maven.org/repository/com/example/library.jar",
            ResourceClass::MinecraftLibrary,
            crate::state::DownloadSourceMode::MirrorPreferred,
        );
        assert_eq!(unmatched.len(), 1);
        assert_eq!(
            unmatched[0].url,
            "https://repo1.maven.org/repository/com/example/library.jar"
        );

        let official_only = resolve_download_routes_for(
            "https://repo1.maven.org/maven2/com/example/library/1/library-1.jar",
            ResourceClass::MinecraftLibrary,
            crate::state::DownloadSourceMode::OfficialOnly,
        );
        assert_eq!(official_only.len(), 1);
        assert_eq!(
            official_only[0].url,
            "https://repo1.maven.org/maven2/com/example/library/1/library-1.jar"
        );
    }

    #[test]
    fn tencent_maven_requires_a_verified_content_hash() {
        assert!(!Integrity::default().has_verified_content_hash());
        assert!(
            !Integrity::md5("0123456789abcdef").has_verified_content_hash()
        );
        assert!(
            Integrity::sha1("0123456789abcdef").has_verified_content_hash()
        );
        assert!(
            Integrity::sha256("0123456789abcdef").has_verified_content_hash()
        );
        assert!(
            Integrity::sha512("0123456789abcdef").has_verified_content_hash()
        );
    }

    #[test]
    fn curseforge_routes_include_safe_mirror_and_direct_fallback() {
        let routes = resolve_download_routes_for(
            "https://api.curseforge.com/v1/mods/search",
            ResourceClass::CurseForge,
            crate::state::DownloadSourceMode::MirrorPreferred,
        );
        assert_eq!(routes.len(), 3);
        assert!(routes[0].is_mirror);
        assert!(!routes[0].allow_sensitive_headers);
        assert_eq!(routes[1].proxy, ProxyPolicy::System);
        assert!(routes[1].allow_sensitive_headers);
        assert_eq!(routes[2].proxy, ProxyPolicy::Direct);
        assert!(routes[2].allow_sensitive_headers);
    }

    #[test]
    fn source_matching_is_origin_safe() {
        assert!(same_origin(
            &Url::parse("https://api.curseforge.com/v1/mods").unwrap(),
            &Url::parse("https://api.curseforge.com/v1/files").unwrap(),
        ));
        assert!(!same_origin(
            &Url::parse("https://api.curseforge.com/v1/mods").unwrap(),
            &Url::parse("https://edge.forgecdn.net/files/1/2/a.jar").unwrap(),
        ));
        assert!(is_sensitive_header("x-api-key"));
        assert!(!header_requires_official_only("x-api-key"));
        assert!(is_sensitive_header("Authorization"));
        assert!(header_requires_official_only("Authorization"));
        assert!(!is_sensitive_header("accept"));
    }

    #[test]
    fn range_segments_require_the_probe_validator() {
        let etag = ResumeMetadata {
            etag: Some("\"fixture\"".to_string()),
            ..ResumeMetadata::default()
        };
        assert!(range_validator_matches(&etag, &etag));
        assert!(!range_validator_matches(&etag, &ResumeMetadata::default()));

        let last_modified = ResumeMetadata {
            last_modified: Some("Tue, 01 Jul 2025 00:00:00 GMT".to_string()),
            ..ResumeMetadata::default()
        };
        assert!(range_validator_matches(&last_modified, &last_modified));
        assert!(!range_validator_matches(
            &last_modified,
            &ResumeMetadata::default()
        ));
    }

    #[tokio::test]
    async fn verifies_streaming_integrity_algorithms() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), b"axolotl download").unwrap();
        let integrity = Integrity {
			size: Some(16),
			sha1: Some("90e438ead880c77ea2d7e726b5aa74e6d21a805f".to_string()),
			sha512: Some("2dcd3e0a9f198e9ef892a28ed6534dd154be2bd13531c961c0852aa6f1e24f633d2cd8288d3cf13c6a482a87c822b74a4901a2aa64292f4a371c5ebfea392c1b".to_string()),
			sha256: Some("120561bc60d59ebe2a08fc229ff2b1eb06b20c4211d21a17c15dd80790f48672".to_string()),
			md5: Some("30018bb52add8c6dbc5d4149c1325df0".to_string()),
			content: ContentValidation::None,
		};
        assert_eq!(verify_file(file.path(), &integrity).await.unwrap(), 16);
    }

    #[tokio::test]
    async fn segmented_download_uses_parallel_validated_ranges() {
        let _guard = RANGE_SPLITTING_TEST_LOCK.lock().await;
        let size = (SEGMENTED_DOWNLOAD_THRESHOLD + 1024 * 1024) as usize;
        let data = Arc::new(
            (0..size)
                .map(|index| (index % 251) as u8)
                .collect::<Vec<_>>(),
        );
        let hash = sha1_smol::Sha1::from(&data[..]).hexdigest();
        let (url, requests, server) =
            spawn_range_server(data.clone(), false).await;
        let route = DownloadRoute {
            url: url.clone(),
            source: DownloadRouteSource::Alternate,
            is_mirror: false,
            allow_sensitive_headers: false,
            supports_range: true,
            proxy: ProxyPolicy::System,
        };
        let request = DownloadRequest::new(&url, ResourceClass::Other)
            .with_integrity(Integrity::sha1(hash).with_size(size as u64));
        let directory = tempfile::tempdir().unwrap();
        let part_path = directory.path().join("fixture.part");
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let semaphore = FetchSemaphore(Semaphore::new(8));
        let outcome = try_segmented_download(
            &request,
            &route,
            size as u64,
            &part_path,
            &semaphore,
            None,
            None,
            &client,
            &client,
        )
        .await;
        match outcome {
            SegmentedDownloadOutcome::Success(result) => {
                assert_eq!(result.size, size as u64);
            }
            _ => panic!("segmented fixture download did not succeed"),
        }
        assert!(requests.load(Ordering::Relaxed) >= 3);
        assert_eq!(
            verify_file(&part_path, &request.integrity).await.unwrap(),
            size as u64
        );
        server.abort();
    }

    #[tokio::test]
    async fn invalid_content_range_disables_host_splitting() {
        let _guard = RANGE_SPLITTING_TEST_LOCK.lock().await;
        let data = Arc::new(vec![7_u8; 1024 * 1024]);
        let (url, _, server) = spawn_range_server(data.clone(), true).await;
        let route = DownloadRoute {
            url: url.clone(),
            source: DownloadRouteSource::Alternate,
            is_mirror: false,
            allow_sensitive_headers: false,
            supports_range: true,
            proxy: ProxyPolicy::System,
        };
        let validator = ResumeMetadata {
            url,
            etag: Some("\"fixture\"".to_string()),
            expected_size: Some(data.len() as u64),
            ..ResumeMetadata::default()
        };
        let directory = tempfile::tempdir().unwrap();
        let part_path = directory.path().join("fixture.part");
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let semaphore = FetchSemaphore(Semaphore::new(4));
        let (progress, _receiver) = tokio::sync::mpsc::unbounded_channel();
        let result = download_segment(
            &route,
            ByteRange {
                index: 0,
                start: 0,
                end: data.len() as u64 - 1,
            },
            data.len() as u64,
            &validator,
            None,
            None,
            None,
            &part_path,
            &semaphore,
            &client,
            &client,
            progress,
        )
        .await;
        assert!(matches!(result, Err(SegmentDownloadError::Protocol)));
        disable_range_splitting(&route);
        assert!(!range_splitting_allowed(&route));
        server.abort();
    }

    #[test]
    fn test_fence_block_after_4_fails() {
        // Update tests if the FenceInner constants change

        let mut fence = FenceInner::new();

        fence.record_fail();
        assert!(!fence.is_blocked());

        fence.record_fail();
        assert!(!fence.is_blocked());

        fence.record_fail();
        assert!(!fence.is_blocked());

        fence.record_fail();
        assert!(fence.is_blocked());
    }

    #[test]
    fn test_fetch_fence_keys_are_independent() {
        let fence = FetchFence {
            inner: Mutex::new(HashMap::new()),
        };

        for _ in 0..FenceInner::FAILURE_THRESHOLD {
            fence.record_fail("/v3/version_file/:sha1/update");
        }

        assert!(fence.is_blocked("/v3/version_file/:sha1/update"));
        assert!(!fence.is_blocked("/v3/project/:id"));
    }

    #[test]
    fn test_fetch_fence_latest_block_minutes() {
        let fence = FetchFence {
            inner: Mutex::new(HashMap::new()),
        };

        {
            let mut inner = fence.inner.lock();
            inner.insert("/expired", FenceInner::new());
            inner.get_mut("/expired").unwrap().block_until =
                Some(Utc::now() - TimeDelta::minutes(1));
            inner.insert("/short", FenceInner::new());
            inner.get_mut("/short").unwrap().block_until =
                Some(Utc::now() + TimeDelta::seconds(61));
            inner.insert("/long", FenceInner::new());
            inner.get_mut("/long").unwrap().block_until =
                Some(Utc::now() + TimeDelta::seconds(140));
        }

        assert_eq!(fence.latest_block_minutes(), 3);
    }

    #[test]
    fn test_fence_block_after_4_fails_with_oks() {
        // Update tests if the FenceInner constants change

        let mut fence = FenceInner::new();

        fence.record_fail();
        assert!(!fence.is_blocked());

        fence.record_fail();
        assert!(!fence.is_blocked());

        fence.record_ok();
        assert!(!fence.is_blocked());

        fence.record_fail();
        assert!(!fence.is_blocked());

        fence.record_fail();
        assert!(fence.is_blocked());
    }

    #[test]
    fn test_fence_not_blocked_after_fails_expire() {
        // Update tests if the FenceInner constants change

        let mut fence = FenceInner::new();

        fence.record_fail();
        assert!(!fence.is_blocked());

        fence.record_fail();
        assert!(!fence.is_blocked());

        fence.prune(Utc::now() + TimeDelta::seconds(60 * 3 + 55)); // Should prune all failures

        fence.record_fail();
        assert!(!fence.is_blocked());

        fence.record_fail();
        assert!(!fence.is_blocked());

        fence.record_fail();
        assert!(!fence.is_blocked());

        fence.record_fail();
        assert!(fence.is_blocked());
    }

    #[test]
    fn test_fence_trigger_block_windows() {
        // brute force flukes
        for i in 0..128 {
            let mut fence = FenceInner::new();

            fence.trigger_block();
            assert!(fence.is_blocked(), "Should be blocked (attempt {i})");

            let block_until = fence.block_until.unwrap();
            assert!(
                block_until > Utc::now() + TimeDelta::seconds(60 + 55),
                "Should be more than 2 minutes (with some leeway) (attempt {i})"
            ); // more than 2 minutes (with some leeway)
            assert!(
                block_until < Utc::now() + TimeDelta::seconds(60 * 5),
                "Should be less than 5 minutes (attempt {i})"
            ); // less than 5 minutes

            fence.block_until = None;

            fence.trigger_block();
            let block_until = fence.block_until.unwrap();
            assert!(
                block_until > Utc::now() + TimeDelta::seconds(60 * 3 + 55),
                "Should be more than 4 minutes (with some leeway) (attempt {i})"
            ); // more than 4 minutes (with some leeway)
            assert!(
                block_until < Utc::now() + TimeDelta::seconds(60 * 10),
                "Should be less than 10 minutes (attempt {i})"
            ); // less than 10 minutes

            fence.block_until = None;

            fence.trigger_block();
            let block_until = fence.block_until.unwrap();
            assert!(
                block_until > Utc::now() + TimeDelta::seconds(60 * 5 + 55),
                "Should be more than 6 minutes (with some leeway) (attempt {i})"
            ); // more than 6 minutes (with some leeway)
            assert!(
                block_until < Utc::now() + TimeDelta::seconds(60 * 15),
                "Should be less than 15 minutes (attempt {i})"
            ); // less than 15 minutes
        }
    }
}

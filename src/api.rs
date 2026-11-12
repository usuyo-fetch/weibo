//! 微博的 HTTP 请求：`ajax/statuses/show`、`ajax/statuses/longtext`，和图片、视频的下载。
//! 请求都经 `usuyo_plugin_kit::http`：每种请求只连自己那几台主机，只连公网地址，从不自动跟跳转。
//! 访客身份（cookie）只发给接口的主机 `weibo.com`；图片和视频的请求不带 cookie。
//! 接口请求不跟跳转：自己读 `Location`，去 `passport.weibo.com` 的就是访客身份不被接受（yt-dlp 的判断）。
//! 下载媒体才跟跳转，每一跳都要在媒体主机名单里。

use std::io::{Read, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use url::Url;
use usuyo_plugin_kit::http::{self, Client, Cookie, Hosts, Policy};
use usuyo_plugin_kit::protocol::{CallFailure, ErrorCode, NotFoundReason, PluginEvent};

use crate::ids::WEB_HOST;

/// 普通桌面 Chrome 的 UA（ParseHub、yt-dlp 的接口请求都带浏览器的 UA）。
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36";
/// 微博的接口和图片、视频都看 Referer（yt-dlp、ParseHub 都带 `https://weibo.com/`）。
const REFERER: &str = "https://weibo.com/";
/// 访客身份被拒时重定向去的主机。
const PASSPORT_HOST: &str = "passport.weibo.com";
const API_TIMEOUT: Duration = Duration::from_secs(20);
/// 一条微博的回答一般几十 KB，留足余量。
const API_BYTES: u64 = 8 << 20;
/// 媒体地址一般跳一次到 CDN。
const MEDIA_REDIRECTS: u32 = 5;
/// 图片、视频只从这些域名的子域下载，和清单的 `network.hosts` 一致。
/// 没有对着平台量过：图片在 `*.sinaimg.cn`（`wx1.sinaimg.cn` 一类）、视频在 `*.weibocdn.com`
/// （`f.video.weibocdn.com` 一类）是 yt-dlp 的测试和 ParseHub 的字段里见到的写法。
/// 不在名单上的主机会记进 `missingReason`，照它补。
const MEDIA_HOST_SUFFIXES: [&str; 2] = [".sinaimg.cn", ".weibocdn.com"];

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("network: {0}")]
    Network(String),
    #[error("HTTP {status}")]
    Http {
        status: u16,
        retry_after: Option<u64>,
        /// 3xx 的 `Location`。
        location: Option<String>,
    },
    /// 接口把请求送去了访客页：站点不收这份访客身份。
    #[error("redirected to the visitor page `{0}`")]
    VisitorRedirect(String),
    #[error("unexpected response: {0}")]
    Shape(String),
    #[error("a response is larger than {limit} bytes")]
    TooLarge { limit: u64 },
    #[error("the host `{0}` is not one of the allowed media hosts")]
    ForbiddenHost(String),
    /// 连之前的检查没过：主机不在这种请求的名单里，或地址不是公网。
    #[error("refused to connect: {0}")]
    Refused(String),
    #[error("cannot write `{path}`: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    /// 平台回了，但不是能用的回答：带着回答的开头，宿主记进失败日志。
    #[error("{error}")]
    Answered {
        error: Box<ApiError>,
        answer: String,
    },
}

impl ApiError {
    /// 要停下来报错的：网络、写盘、限流、被拦、访客身份不被接受。其余在可选的那一步里只记进 `missing`，主体照样交出去。
    pub fn is_hard(&self) -> bool {
        match self {
            ApiError::Network(_) | ApiError::Io { .. } | ApiError::VisitorRedirect(_) => true,
            ApiError::Http { status, .. } => matches!(status, 403 | 429),
            ApiError::Shape(_)
            | ApiError::TooLarge { .. }
            | ApiError::ForbiddenHost(_)
            | ApiError::Refused(_) => false,
            ApiError::Answered { error, .. } => error.is_hard(),
        }
    }

    pub fn answered(self, raw: &[u8]) -> ApiError {
        let end = raw.len().min(4096);
        ApiError::Answered {
            error: Box::new(self),
            answer: String::from_utf8_lossy(&raw[..end]).into_owned(),
        }
    }
}

impl From<ApiError> for CallFailure {
    fn from(err: ApiError) -> CallFailure {
        failure(&err)
    }
}

fn failure(err: &ApiError) -> CallFailure {
    let message = match err {
        ApiError::Http {
            status,
            location: Some(l),
            ..
        } => format!("HTTP {status} redirecting to {l}"),
        _ => err.to_string(),
    };
    match err {
        ApiError::Network(_) => CallFailure::new(ErrorCode::Network, message),
        ApiError::VisitorRedirect(_) => visitor_rejected(message),
        ApiError::Http {
            status,
            retry_after,
            location,
        } => match status {
            403 | 418 => blocked(message),
            // 跳去验证页的算被拦；别的跳转是接口改了
            300..=399
                if location.as_deref().is_some_and(|l| {
                    let l = l.to_ascii_lowercase();
                    l.contains("captcha") || l.contains("geetest")
                }) =>
            {
                blocked(message)
            }
            // 404 不说是不是删了：接口没有说
            404 => CallFailure::new(ErrorCode::NotFound, message),
            429 => CallFailure {
                retry_after: *retry_after,
                hint: Some("微博提示请求太频繁，请过一会儿再试".into()),
                ..CallFailure::new(ErrorCode::RateLimited, message)
            },
            500..=599 => CallFailure::new(ErrorCode::Network, message),
            _ => CallFailure::new(ErrorCode::UpstreamChanged, message),
        },
        ApiError::Shape(_)
        | ApiError::TooLarge { .. }
        | ApiError::ForbiddenHost(_)
        | ApiError::Refused(_) => CallFailure::new(ErrorCode::UpstreamChanged, message),
        ApiError::Io { .. } => CallFailure::new(ErrorCode::Internal, message),
        ApiError::Answered { error, answer } => failure(error).with_detail(answer),
    }
}

pub fn blocked(message: String) -> CallFailure {
    CallFailure {
        hint: Some(
            "微博出现了验证页或风控提示；请稍后再试，也可以先在浏览器里打开微博完成验证".into(),
        ),
        ..CallFailure::new(ErrorCode::Blocked, message)
    }
}

/// 访客身份没有交来，或交来的微博不收。
pub fn visitor_rejected(message: String) -> CallFailure {
    CallFailure {
        hint: Some("微博不收这份访客身份：在「获取插件」页的微博下面换一份（不是你的账号）".into()),
        site: Some(WEB_HOST.into()),
        ..CallFailure::new(ErrorCode::VisitorRequired, message)
    }
}

/// 一次也没有拿到访客身份：不发请求就回。
pub fn visitor_missing() -> CallFailure {
    CallFailure {
        hint: Some(
            "这种方法要一份访客身份：在「获取插件」页的微博下面取一份（不是你的账号）".into(),
        ),
        site: Some(WEB_HOST.into()),
        ..CallFailure::new(
            ErrorCode::VisitorRequired,
            "no visitor identity was handed over (USUYO_VISITOR is not set)".to_owned(),
        )
    }
}

/// 要连的主机和地址策略。正式运行用 `production`；测试把它指到本机的假服务器。
#[derive(Debug, Clone)]
pub struct Endpoints {
    api_base: Url,
    /// 测试用的本机主机：算接口主机，也算允许的媒体主机，媒体链接不升级成 https。
    local_host: Option<String>,
    policy: Policy,
}

impl Endpoints {
    pub fn production() -> Self {
        Self {
            api_base: Url::parse(&format!("https://{WEB_HOST}/")).expect("constant URL"),
            local_host: None,
            policy: Policy::from_host(),
        }
    }

    /// 测试用：`base` 是 `http://127.0.0.1:<端口>`，`policy` 让这个本机地址算公网。
    pub fn local(base: &str, policy: Policy) -> Self {
        let api_base = Url::parse(base).expect("local base URL");
        let local_host = api_base.host_str().map(str::to_owned);
        Self {
            api_base,
            local_host,
            policy,
        }
    }

    /// 访客身份只发给这些主机。
    pub fn api_hosts(&self) -> Vec<String> {
        self.api_base
            .host_str()
            .map(str::to_owned)
            .into_iter()
            .collect()
    }

    fn is_media_host(&self, host: &str) -> bool {
        self.local_host.as_deref() == Some(host)
            || MEDIA_HOST_SUFFIXES
                .iter()
                .any(|s| host.len() > s.len() && host.ends_with(s))
    }

    fn client(&self, hosts: &[&str]) -> Client {
        let local = self.local_host.as_deref();
        Client::new(USER_AGENT, Hosts::only(hosts.iter().copied().chain(local)))
            .with_policy(self.policy)
    }

    /// 宿主交来的访客身份（`USUYO_VISITOR`），只发给接口的主机、只走 https。没交时是 `None`。
    pub fn visitor_cookie(&self) -> Result<Option<Cookie>, http::VisitorError> {
        Cookie::visitor(self.api_hosts())
    }

    /// 页面给的媒体地址：补 scheme、升级成 https、核对主机。
    pub fn media_url(&self, raw: &str) -> Result<Url, ApiError> {
        let raw = raw.trim();
        let full = if raw.starts_with("//") {
            format!("https:{raw}")
        } else {
            raw.to_owned()
        };
        let mut url = Url::parse(&full)
            .map_err(|_| ApiError::Shape("a media address is not a URL".into()))?;
        let host = url
            .host_str()
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        if !self.is_media_host(&host) {
            return Err(ApiError::ForbiddenHost(host));
        }
        let local = self.local_host.as_deref() == Some(host.as_str());
        // 清单的 `network.hosts` 没写端口，只放行默认端口
        if !local && url.port().is_some() {
            return Err(ApiError::ForbiddenHost(format!(
                "{host}:{}",
                url.port().unwrap_or_default()
            )));
        }
        if url.scheme() == "http" && !local {
            url.set_scheme("https")
                .map_err(|()| ApiError::Shape("a media address cannot use https".into()))?;
        }
        if url.scheme() != "https" && !local {
            return Err(ApiError::Shape("a media address is not http(s)".into()));
        }
        Ok(url)
    }
}

fn transport(e: http::Error) -> ApiError {
    match e {
        http::Error::TooLarge(limit) => ApiError::TooLarge { limit },
        e if e.is_refusal() => ApiError::Refused(e.to_string()),
        e => ApiError::Network(e.to_string()),
    }
}

fn status_error(response: &http::Response) -> ApiError {
    ApiError::Http {
        status: response.status(),
        retry_after: response
            .header("retry-after")
            .and_then(|v| v.trim().parse().ok()),
        location: response.header("location").map(str::to_owned),
    }
}

pub struct Weibo {
    endpoints: Endpoints,
}

impl Weibo {
    pub fn new(endpoints: Endpoints) -> Self {
        Self { endpoints }
    }

    pub fn endpoints(&self) -> &Endpoints {
        &self.endpoints
    }

    /// `ajax/statuses/show`：`id` 是 `mid` 或 `mblogid`（yt-dlp 把链接里的那一段原样放进 `id`），
    /// `isGetLongText=true` 照 ParseHub。返回回答的原始字节。
    pub fn show(&self, id: &str, cookie: &Cookie) -> Result<Vec<u8>, ApiError> {
        step("微博");
        self.api(
            "ajax/statuses/show",
            &[("id", id), ("isGetLongText", "true")],
            cookie,
        )
    }

    /// `ajax/statuses/longtext`：参数 `id` 是 `mblogid`（Weibo-archiver 的 `longText(postMBlogId)`）。
    pub fn longtext(&self, mblogid: &str, cookie: &Cookie) -> Result<Vec<u8>, ApiError> {
        step("长文");
        self.api("ajax/statuses/longtext", &[("id", mblogid)], cookie)
    }

    fn api(
        &self,
        path: &str,
        query: &[(&str, &str)],
        cookie: &Cookie,
    ) -> Result<Vec<u8>, ApiError> {
        let mut url = self
            .endpoints
            .api_base
            .join(path)
            .map_err(|e| ApiError::Shape(format!("bad API URL: {e}")))?;
        url.query_pairs_mut().extend_pairs(query);
        let host = self.endpoints.api_hosts();
        let hosts: Vec<&str> = host.iter().map(String::as_str).collect();
        let mut response = self
            .endpoints
            .client(&hosts)
            .get(&url)
            .header("Accept", "application/json, text/plain, */*")
            .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.6")
            .header("Referer", REFERER)
            .cookie(cookie)
            .timeout(API_TIMEOUT)
            .send()
            .map_err(transport)?;
        let status = response.status();
        if (300..400).contains(&status) {
            let location = response.header("location").map(str::to_owned);
            let head = format!(
                "HTTP {status}\nLocation: {}",
                location.as_deref().unwrap_or("")
            );
            let to_passport = location.as_deref().is_some_and(|l| {
                url.join(l)
                    .ok()
                    .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
                    .is_some_and(|h| h == PASSPORT_HOST)
            });
            let error = if to_passport {
                ApiError::VisitorRedirect(location.unwrap_or_default())
            } else {
                status_error(&response)
            };
            return Err(error.answered(head.as_bytes()));
        }
        if !(200..300).contains(&status) {
            let e = status_error(&response);
            let head = response.read_to_vec(4096).unwrap_or_default();
            return Err(e.answered(&head));
        }
        response.read_to_vec(API_BYTES).map_err(transport)
    }

    /// 把一个文件流式存到 `dest`，最多 `limit` 字节。跟随跳转，每一跳都要在媒体主机名单里。
    /// 不带 cookie。每秒向宿主报一次进度。
    pub fn download(
        &self,
        url: &Url,
        dest: &Path,
        limit: u64,
        timeout: Duration,
        label: &str,
    ) -> Result<u64, ApiError> {
        let response = self
            .endpoints
            .client(&MEDIA_HOST_SUFFIXES)
            .get(url)
            .header("Referer", REFERER)
            .follow_redirects(MEDIA_REDIRECTS)
            .timeout(timeout)
            .send()
            .map_err(transport)?;
        if !(200..300).contains(&response.status()) {
            return Err(status_error(&response));
        }
        let total = response
            .header("content-length")
            .and_then(|v| v.parse::<u64>().ok());
        if total.is_some_and(|t| t > limit) {
            return Err(ApiError::TooLarge { limit });
        }
        let io = |source| ApiError::Io {
            path: dest.display().to_string(),
            source,
        };
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        let file = std::fs::File::create_new(dest).map_err(io)?;
        let written = copy_limited(response.into_reader(limit), file, limit, total, label);
        if written.is_err() {
            // 写了一半的文件不留在输出目录里：宿主会把没被引用的文件也原样收下。
            // 清理失败不改变这次调用的结果，调用失败时整个目录本来就会被丢掉。
            let _ = std::fs::remove_file(dest);
        }
        written.map_err(|e| match e {
            CopyError::Api(e) => e,
            CopyError::Io(source) => io(source),
        })
    }
}

enum CopyError {
    Api(ApiError),
    Io(std::io::Error),
}

/// 把响应体流式写进文件，`reader` 读出超过 `limit` 字节时报错，每秒报一次进度。
fn copy_limited(
    mut reader: impl Read,
    mut file: std::fs::File,
    limit: u64,
    total: Option<u64>,
    label: &str,
) -> Result<u64, CopyError> {
    let mut buffer = vec![0u8; 64 << 10];
    let mut written = 0u64;
    let mut last_report = Instant::now();
    loop {
        let n = reader.read(&mut buffer).map_err(|e| {
            CopyError::Api(if e.kind() == std::io::ErrorKind::FileTooLarge {
                ApiError::TooLarge { limit }
            } else {
                ApiError::Network(e.to_string())
            })
        })?;
        if n == 0 {
            break;
        }
        written += n as u64;
        file.write_all(&buffer[..n]).map_err(CopyError::Io)?;
        if last_report.elapsed() >= Duration::from_secs(1) {
            progress(written, total, label);
            last_report = Instant::now();
        }
    }
    file.flush().map_err(CopyError::Io)?;
    progress(written, total, label);
    Ok(written)
}

/// 开始一步之前告诉宿主：一步一步慢下来时，宿主照样收得到 progress，不按「没进度」杀掉进程。
pub(crate) fn step(label: &str) {
    let event = PluginEvent::Progress {
        done: None,
        total: None,
        unit: None,
        message: Some(label.into()),
    };
    eprintln!("{}", event.stderr_line());
}

fn progress(done: u64, total: Option<u64>, label: &str) {
    let event = PluginEvent::Progress {
        done: Some(done as f64),
        total: total.map(|t| t as f64),
        unit: Some("B".into()),
        message: Some(label.into()),
    };
    eprintln!("{}", event.stderr_line());
}

/// `not-found` 的理由由调用方（回答里的话）决定，这里只留一个构造，免得各处拼。
pub fn not_found(reason: Option<NotFoundReason>, message: String) -> CallFailure {
    CallFailure {
        reason,
        ..CallFailure::new(ErrorCode::NotFound, message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_hosts() {
        let e = Endpoints::production();
        let ok = |s: &str| e.media_url(s).map(|u| u.to_string());
        assert_eq!(
            ok("http://f.video.weibocdn.com/o0/a.mp4?label=mp4_hd").unwrap(),
            "https://f.video.weibocdn.com/o0/a.mp4?label=mp4_hd"
        );
        assert!(ok("https://wx1.sinaimg.cn/large/a.jpg").is_ok());
        assert!(ok("//wx2.sinaimg.cn/large/a.jpg").is_ok());
        assert!(matches!(
            ok("https://wx1.sinaimg.cn:8443/a"),
            Err(ApiError::ForbiddenHost(_))
        ));
        for bad in [
            "https://sinaimg.cn/a",
            "https://evilsinaimg.cn/a",
            "https://sinaimg.cn.evil.test/a",
            "https://weibo.com/a",
            "https://127.0.0.1/a",
        ] {
            assert!(matches!(ok(bad), Err(ApiError::ForbiddenHost(_))), "{bad}");
        }
        assert!(matches!(
            ok("ftp://wx1.sinaimg.cn/a"),
            Err(ApiError::Shape(_))
        ));
        assert!(matches!(ok("nonsense"), Err(ApiError::Shape(_))));
    }

    #[test]
    fn failure_mapping() {
        let http = |status, location: Option<&str>| {
            CallFailure::from(ApiError::Http {
                status,
                retry_after: Some(7),
                location: location.map(str::to_owned),
            })
        };
        assert_eq!(http(403, None).code, ErrorCode::Blocked);
        assert_eq!(http(429, None).retry_after, Some(7));
        assert_eq!(http(502, None).code, ErrorCode::Network);
        assert_eq!(http(404, None).code, ErrorCode::NotFound);
        assert_eq!(http(404, None).reason, None);
        assert_eq!(
            http(302, Some("https://weibo.com/somewhere")).code,
            ErrorCode::UpstreamChanged
        );
        assert_eq!(
            http(302, Some("https://captcha.weibo.com/verify")).code,
            ErrorCode::Blocked
        );
        let visitor = CallFailure::from(
            ApiError::VisitorRedirect("https://passport.weibo.com/visitor/visitor".into())
                .answered(b"HTTP 302\nLocation: https://passport.weibo.com/visitor/visitor"),
        );
        assert_eq!(visitor.code, ErrorCode::VisitorRequired);
        assert!(visitor.detail.is_some());
    }
}

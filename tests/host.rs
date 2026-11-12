//! 真的二进制：不需要网络的子命令，访客身份怎么进来，以及清单经宿主的校验和调用。
//! 起进程时清空环境，代理指向一个关着的本机端口，万一哪条路径真的发了请求，只会失败，不会连到微博。

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use usuyo_plugin::protocol::{Access, Account, Canonicalized, ErrorCode, Identity, Login};
use usuyo_plugin::{
    CallLimits, CallOptions, FetchInput, FetchRequest, HostConfig, PluginError, PluginHost,
    discover,
};

const BIN: &str = env!("CARGO_BIN_EXE_usuyo-fetch-weibo");
const DEAD_PROXY: &str = "http://127.0.0.1:9";
const MID: &str = "3501756485200075";
const BID: &str = "z0JH2lOMb";

fn run_with(args: &[&str], visitor: Option<&Path>) -> (i32, Value, String) {
    let mut cmd = Command::new(BIN);
    cmd.env_clear()
        .env("USUYO_PROTOCOL", "1")
        .env("USUYO_LOCALE", "zh-CN")
        .env("HTTP_PROXY", DEAD_PROXY)
        .env("HTTPS_PROXY", DEAD_PROXY)
        .env("ALL_PROXY", DEAD_PROXY)
        .args(args);
    if let Some(v) = visitor {
        cmd.env("USUYO_VISITOR", v);
    }
    let output = cmd.output().unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json = if stdout.trim().is_empty() {
        Value::Null
    } else {
        assert_eq!(
            stdout.lines().count(),
            1,
            "stdout is one JSON value: {stdout}"
        );
        serde_json::from_str(&stdout).unwrap()
    };
    (
        output.status.code().unwrap(),
        json,
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn run(args: &[&str]) -> (i32, Value) {
    let (code, json, _) = run_with(args, None);
    (code, json)
}

#[test]
fn canonicalize_needs_no_network() {
    for link in [
        format!("https://weibo.com/1700000001/{BID}"),
        format!("https://www.weibo.com/1700000001/{BID}?refer_flag=1"),
        format!("https://weibo.com/detail/{MID}"),
        format!("https://m.weibo.cn/status/{BID}"),
        format!("https://m.weibo.cn/detail/{MID}"),
    ] {
        let (code, out) = run(&["canonicalize", &link]);
        assert_eq!(code, 0, "{link}");
        assert_eq!(out["match"], true, "{link}");
        assert_eq!(out["identity"]["platform"], "weibo");
        assert_eq!(out["identity"]["contentId"], MID, "{link}");
        assert_eq!(
            out["identity"]["url"],
            format!("https://m.weibo.cn/detail/{MID}")
        );
        assert!(out.get("resolvedUrl").is_none());
    }
    // 油猴脚本交来的链接：`https://weibo.com/` 加记下的 `<作者 id>/<mblogid>`（脚本测试里的收藏样例）
    let (_, out) = run(&["canonicalize", "https://weibo.com/1700000001/Qa1Bb2Cc3"]);
    assert_eq!(out["identity"]["contentId"], "5223894290623475");
    for link in [
        "https://weibo.com/u/1700000001",
        "https://example.com/1700000001/z0JH2lOMb",
        "https://m.weibo.cn/u/1700000001",
    ] {
        let (code, out) = run(&["canonicalize", link]);
        assert_eq!((code, &out["match"]), (0, &Value::Bool(false)), "{link}");
    }
}

#[test]
fn health_does_not_touch_the_network() {
    for args in [&["health"][..], &["health", "--probe"][..]] {
        let (code, out) = run(args);
        assert_eq!(code, 0);
        assert_eq!(
            (out["ready"].as_bool(), out["protocol"].as_u64()),
            (Some(true), Some(1))
        );
        assert_eq!(out["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(out["login"], serde_json::json!([]));
        assert_eq!(out["checks"], serde_json::json!([]));
    }
}

#[test]
fn bad_usage_exits_2() {
    for args in [
        &[][..],
        &["nope"][..],
        &["fetch", "x"][..],
        &["fetch", "x", "--out", "/o"][..],
        &["sources"][..],
        &["canonicalize"][..],
    ] {
        let (code, out) = run(args);
        assert_eq!((code, out), (2, Value::Null), "{args:?}");
    }
}

#[test]
fn each_call_goes_to_its_method() {
    // 方法和调用对不上的回 invalid-args，不发请求
    let link = format!("https://weibo.com/1700000001/{BID}");
    for args in [
        &["sources", "--method", "status"][..],
        &["list", "weibo:favorites", "--method", "favorites"][..],
        &["fetch", &link, "--out", "/o", "--method", "userscript"][..],
        &["fetch", &link, "--out", "/o", "--method", "favorites"][..],
        &["fetch", &link, "--out", "/o", "--method", "nope"][..],
    ] {
        let (code, out) = run(args);
        assert_eq!(
            (code, &out["error"]["code"]),
            (1, &Value::from("invalid-args")),
            "{args:?}"
        );
    }
}

/// 宿主交来的访客身份文件（`USUYO_VISITOR`，和 `USUYO_CREDENTIALS` 一个形状）。
fn visitor_file(dir: &Path, value: &str) -> PathBuf {
    let path = dir.join("visitor.json");
    let file = serde_json::json!({
        "sites": [{"site": "weibo.com", "source": "visitor-window"}],
        "storageState": {"cookies": [
            {"name": "SUB", "value": value, "domain": ".weibo.com", "path": "/"}]}
    });
    std::fs::write(&path, file.to_string()).unwrap();
    path
}

#[test]
fn fetch_without_a_visitor_identity_answers_visitor_required_offline() {
    let dir = tempfile::tempdir().unwrap();
    let link = format!("https://weibo.com/1700000001/{BID}");
    let (code, out, _) = run_with(
        &[
            "fetch",
            &link,
            "--out",
            dir.path().to_str().unwrap(),
            "--method",
            "status",
        ],
        None,
    );
    assert_eq!(code, 1);
    assert_eq!(out["error"]["code"], "visitor-required");
    assert_eq!(out["error"]["site"], "weibo.com");
    assert!(out["error"]["hint"].as_str().unwrap().contains("访客身份"));
    assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none());

    // 不是微博的链接先答 unsupported-input
    let (_, out, _) = run_with(
        &[
            "fetch",
            "https://example.com/x",
            "--out",
            dir.path().to_str().unwrap(),
            "--method",
            "status",
        ],
        None,
    );
    assert_eq!(out["error"]["code"], "unsupported-input");
}

#[test]
fn a_malformed_visitor_identity_is_reported_without_its_value() {
    let dir = tempfile::tempdir().unwrap();
    let visitor = visitor_file(dir.path(), "visitor-secret-1\r\nHost: evil");
    let out_dir = dir.path().join("out");
    std::fs::create_dir(&out_dir).unwrap();
    let link = format!("https://weibo.com/1700000001/{BID}");
    let (code, out, stderr) = run_with(
        &[
            "fetch",
            &link,
            "--out",
            out_dir.to_str().unwrap(),
            "--method",
            "status",
        ],
        Some(&visitor),
    );
    assert_eq!(code, 1);
    assert_eq!(out["error"]["code"], "internal");
    let text = format!("{out} {stderr}");
    assert!(!text.contains("visitor-secret-1"), "{text}");
    assert!(std::fs::read_dir(&out_dir).unwrap().next().is_none());
}

/// 清单装成宿主认的样子：`<root>/usuyo-fetch/weibo/<version>/`。
fn host() -> (tempfile::TempDir, PluginHost) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("plugins/usuyo-fetch/weibo/0.1.0");
    std::fs::create_dir_all(&dir).unwrap();
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("usuyo-plugin.yaml");
    std::fs::copy(manifest, dir.join("usuyo-plugin.yaml")).unwrap();
    std::fs::copy(BIN, dir.join("usuyo-fetch-weibo")).unwrap();
    let found = discover(&tmp.path().join("plugins")).unwrap();
    assert!(found.rejected.is_empty(), "{:?}", found.rejected);
    let host = PluginHost::new(
        HostConfig {
            work_dir: tmp.path().join("work"),
            cache_dir: tmp.path().join("cache"),
            locale: "zh-CN".into(),
            limits: CallLimits::default(),
        },
        found.fetch,
    );
    (tmp, host)
}

#[tokio::test]
async fn the_manifest_loads_and_declares_every_row_of_routes_section_9() {
    let (_tmp, host) = host();
    let plugin = &host.plugins()[0];
    assert_eq!(plugin.id().as_str(), "usuyo-fetch/weibo");
    assert_eq!(plugin.header.version.to_string(), env!("CARGO_PKG_VERSION"));
    let spec = &plugin.spec;
    let ids: Vec<&str> = spec.methods.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["status", "userscript", "favorites"]);

    let status = spec.method("status").unwrap();
    assert_eq!(
        (status.access, status.login, status.account),
        (Access::PublicWeb, Login::None, Account::Visitor)
    );
    assert!(status.built && status.sites.is_empty() && status.capabilities.fetch.is_some());
    assert_eq!(status.visitors.len(), 1);
    assert!(status.on_by_default());

    let script = spec.method("userscript").unwrap();
    assert_eq!(
        (script.access, script.login, script.account),
        (Access::BrowserSession, Login::Required, Account::OwnBrowser)
    );
    assert!(
        script.built && script.capabilities.fetch.is_none() && script.capabilities.list.is_none()
    );

    let favorites = spec.method("favorites").unwrap();
    assert_eq!(
        (favorites.access, favorites.login, favorites.account),
        (Access::SessionApi, Login::Required, Account::InUsuyo)
    );
    assert!(!favorites.built);

    for hit in [
        format!("https://weibo.com/1700000001/{BID}"),
        format!("https://www.weibo.com/1700000001/{BID}"),
        format!("https://weibo.com/detail/{MID}"),
        format!("https://m.weibo.cn/status/{MID}"),
        format!("https://m.weibo.cn/detail/{BID}"),
    ] {
        let url = url::Url::parse(&hit).unwrap();
        assert_eq!(host.candidates(&url).len(), 1, "{hit}");
    }
    for miss in [
        "https://www.xiaohongshu.com/explore/x",
        "https://example.com/1/2",
    ] {
        let url = url::Url::parse(miss).unwrap();
        assert!(host.candidates(&url).is_empty(), "{miss}");
    }

    // 经宿主调一次：输出要过宿主的 schema 校验
    let url = url::Url::parse(&format!("https://weibo.com/1700000001/{BID}")).unwrap();
    let canonical = host
        .canonicalize(host.candidates(&url)[0], &url, &CallOptions::default())
        .await
        .unwrap();
    let Canonicalized::Match {
        identity: Identity::Platform { content_id, .. },
        ..
    } = canonical
    else {
        panic!("{canonical:?}")
    };
    assert_eq!(content_id, MID);
}

#[tokio::test]
async fn a_link_that_is_not_a_post_is_turned_away_through_the_host() {
    let (_tmp, host) = host();
    let plugin = &host.plugins()[0];
    // 主机名和两段路径对得上 `match`，认不出是微博：插件回 unsupported-input
    let url = url::Url::parse("https://weibo.com/ttarticle/p").unwrap();
    let request = FetchRequest {
        method: "status".into(),
        input: FetchInput::Url(url),
        want: None,
        input_file: None,
    };
    let err = host
        .fetch(plugin, request, &CallOptions::default())
        .await
        .unwrap_err();
    let PluginError::Call { failure, .. } = err else {
        panic!("{err:?}")
    };
    assert_eq!(failure.code, ErrorCode::UnsupportedInput);
}

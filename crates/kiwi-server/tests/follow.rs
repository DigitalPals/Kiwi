//! Tweak updates: Kiwi finds newer tweak releases on (a local) GitHub, builds them, keeps
//! them when they build and falls back when they don't.
mod common;

use axum::{
    Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use common::*;
use reqwest::Method;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, atomic::Ordering};

const RECIPE_SHA: &str = "c0365e2af10df1c41f59b047122aa0291c40236f";
const FAILING: char = 'e';

#[derive(Default)]
struct GitHub {
    releases: Mutex<Value>,
    /// Commits of tags.
    tags: Mutex<Vec<(String, String)>>,
    /// The compare status of every range.
    compare: Mutex<String>,
}

async fn releases(State(gh): State<Arc<GitHub>>) -> impl IntoResponse {
    axum::Json(gh.releases.lock().unwrap().clone())
}

async fn tag_commit(State(gh): State<Arc<GitHub>>, Path(tag): Path<String>) -> impl IntoResponse {
    match gh.tags.lock().unwrap().iter().find(|(t, _)| *t == tag) {
        Some((_, sha)) => axum::Json(json!({"sha": sha,
            "commit": {"committer": {"date": "2026-11-10T11:26:42Z"}}}))
        .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn compare(State(gh): State<Arc<GitHub>>) -> impl IntoResponse {
    axum::Json(json!({"status": *gh.compare.lock().unwrap()}))
}

async fn github() -> (Arc<GitHub>, String) {
    let gh = Arc::new(GitHub::default());
    *gh.compare.lock().unwrap() = "ahead".into();
    *gh.releases.lock().unwrap() = json!([]);
    let app = Router::new()
        .route("/repos/and101/RedditFilter/releases", get(releases))
        .route("/repos/and101/RedditFilter/commits/{tag}", get(tag_commit))
        .route("/repos/and101/RedditFilter/compare/{range}", get(compare))
        .with_state(gh.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (gh, base)
}

fn release(gh: &GitHub, tag: &str, sha: &str) {
    gh.releases
        .lock()
        .unwrap()
        .as_array_mut()
        .unwrap()
        .insert(0, json!({"tag_name": tag}));
    gh.tags.lock().unwrap().push((tag.into(), sha.into()));
}

/// An iOS runner that builds the version the pins name, records each request and fails
/// to compile sources whose commit is all `e`.
fn fake_runner(k: &TestKiwi) {
    let script = k.dir.path().join("fake-builder.py");
    let text = format!(
        r#"
import sys,json,pathlib,zipfile,plistlib,hashlib
command=sys.argv[1]
root=pathlib.Path(sys.argv[sys.argv.index('--data-dir')+1])
request=json.loads(pathlib.Path(sys.argv[sys.argv.index('--request')+1]).read_text())
with open(root/'requests.jsonl','a') as f: f.write(json.dumps(dict(command=command,request=request))+'\n')
def emit(kind,**data):print(json.dumps(dict(event=kind,**data)),flush=True)
if command=='plan':
 emit('health',checks=[dict(name='Fixture',ready=True)],ready=True)
 pins=request.get('pins') or {{}}
 if '{failing}'*40 in (pins.get('packages') or {{}}).values():
  emit('error',message='The redditfilter tweak did not compile.',blocked=False); sys.exit(1)
 version=pins.get('app_version','2026.45.0')
 key=hashlib.sha256(json.dumps([request['tweaks'],pins],sort_keys=True).encode()).hexdigest()
 emit('plan',plan=dict(key=key,version=version,external_id='1234',tweaks=request['tweaks'],name=None,pins=pins))
else:
 version=request['plan']['version']
 p=root/'automation'/'candidate.ipa'
 info=dict(CFBundleIdentifier='com.reddit.Reddit',CFBundleShortVersionString=version,CFBundleVersion=version,KiwiBuildRevision=request['sequence'],MinimumOSVersion='17.0',CFBundleSupportedPlatforms=['iPhoneOS'],UIDeviceFamily=[1])
 with zipfile.ZipFile(p,'w') as z:
  z.writestr('Payload/Reddit.app/Info.plist',plistlib.dumps(info))
  z.writestr('Payload/Reddit.app/Reddit',b'x'*300000)
 sha=hashlib.sha256(p.read_bytes()).hexdigest()
 dest=root/'automation'/'artifacts'
 dest.mkdir(exist_ok=True)
 p.rename(dest/(sha+'.ipa'))
 emit('artifact',sha256=sha,version=version,build=version,revision=request['sequence'],min_os='17.0',size=(dest/(sha+'.ipa')).stat().st_size,notes='Fixture')
"#,
        failing = FAILING
    );
    std::fs::write(&script, text).unwrap();
    *k.kiwi.automation.runner.write().unwrap() = script;
    k.kiwi.automation.builds_ios.store(true, Ordering::SeqCst);
}

/// The pins of the last plan request.
fn last_pins(k: &TestKiwi) -> Value {
    let log = std::fs::read_to_string(k.dir.path().join("data/requests.jsonl")).unwrap();
    let last: Value = log
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .rfind(|r| r["command"] == "plan")
        .unwrap();
    last["request"]["pins"].clone()
}

async fn finished(k: &TestKiwi) -> Value {
    let phone = k.phone();
    for _ in 0..300 {
        let s = phone.ok("/api/automation/reddit").await;
        if s["jobs"][0]["finished_at"].is_string() {
            return s;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("the Reddit build did not finish");
}

#[tokio::test]
async fn newer_tweak_releases_build_by_themselves_and_failures_keep_what_built() {
    let k = TestKiwi::start().await;
    let (gh, base) = github().await;
    let follower = &k.kiwi.automation.follow;
    follower.background.store(false, Ordering::SeqCst);
    *follower.github_api.write().unwrap() = base;
    fake_runner(&k);
    let phone = k.phone();
    phone
        .send(
            Method::PATCH,
            "/api/settings",
            Some(json!({"kiwi_builds": true})),
        )
        .await;
    let initial = phone.ok("/api/automation/reddit").await;
    assert_eq!(initial["follow_tweak_updates"], true);
    assert_eq!(initial["tweak_sources"][0]["follows"], "release");
    assert_eq!(initial["tweak_sources"][0]["sha"], RECIPE_SHA);
    assert_eq!(initial["tweak_sources"][0]["updated"], false);
    phone
        .post("/api/automation/reddit/add", initial["preferences"].clone())
        .await;
    let first = finished(&k).await;
    assert_eq!(first["published"]["version"], "2026.45.0", "{first}");
    assert!(last_pins(&k).is_null(), "the recipe's own sources");

    // Nothing newer upstream: nothing to build.
    release(&gh, "v2026.45.0-1.3.8", RECIPE_SHA);
    kiwi_server::automation::follow::check(&k.kiwi).await;
    let state = phone.ok("/api/automation/reddit").await;
    assert!(state["follow"]["checked_at"].is_string());
    assert!(state["follow"]["candidate"].is_null(), "{state}");

    // A release for a newer Reddit builds as soon as Kiwi finds it.
    release(&gh, "v2026.46.0-1.3.9", &"a".repeat(40));
    kiwi_server::automation::follow::check(&k.kiwi).await;
    assert_eq!(
        phone.ok("/api/automation/reddit").await["follow"]["candidate"]["redditfilter"]["tag"],
        "v2026.46.0-1.3.9"
    );
    kiwi_server::automation::tick(&k.kiwi);
    let built = finished(&k).await;
    assert_eq!(built["jobs"][0]["status"], "ready", "{built}");
    assert_eq!(built["jobs"][0]["trial"], true);
    assert_eq!(built["published"]["version"], "2026.46.0");
    assert_eq!(built["tweak"], "RedditFilter 1.3.9");
    assert_eq!(built["app_version"], "2026.46.0");
    assert!(built["follow"]["candidate"].is_null());
    assert_eq!(built["tweak_sources"][0]["updated"], true);
    assert_eq!(built["tweak_sources"][0]["version"], "1.3.9");
    assert_eq!(
        last_pins(&k),
        json!({"packages": {"redditfilter": "a".repeat(40)}, "app_version": "2026.46.0",
               "tweak": "RedditFilter 1.3.9"})
    );
    // It isn't due again until its next check.
    kiwi_server::automation::tick(&k.kiwi);
    assert_eq!(finished(&k).await["sequence"], built["sequence"]);

    // A release that doesn't build is tried twice, then given up; Kiwi keeps 1.3.9.
    release(&gh, "v2026.47.0-1.4.0", &FAILING.to_string().repeat(40));
    kiwi_server::automation::follow::check(&k.kiwi).await;
    kiwi_server::automation::tick(&k.kiwi);
    let failed = finished(&k).await;
    assert_eq!(failed["jobs"][0]["status"], "failed", "{failed}");
    assert_eq!(failed["follow"]["attempts"], 1);
    phone.post("/api/automation/reddit/check", json!({})).await;
    let given_up = finished(&k).await;
    assert_eq!(given_up["jobs"][0]["status"], "failed");
    assert!(given_up["follow"]["candidate"].is_null(), "{given_up}");
    assert_eq!(
        given_up["follow"]["failed"]["pins"]["redditfilter"]["version"],
        "1.4.0"
    );
    assert!(
        given_up["follow"]["failed"]["error"]
            .as_str()
            .unwrap()
            .contains("did not compile")
    );
    assert_eq!(given_up["tweak"], "RedditFilter 1.3.9");
    assert_eq!(given_up["published"]["version"], "2026.46.0");
    phone.post("/api/automation/reddit/check", json!({})).await;
    assert_eq!(finished(&k).await["jobs"][0]["status"], "unchanged");
    assert_eq!(last_pins(&k)["packages"]["redditfilter"], "a".repeat(40));
    // Found again, the same release isn't retried.
    kiwi_server::automation::follow::check(&k.kiwi).await;
    assert!(phone.ok("/api/automation/reddit").await["follow"]["candidate"].is_null());

    // Rewritten history is not followed.
    *gh.compare.lock().unwrap() = "diverged".into();
    release(&gh, "v2026.48.0-1.4.1", &"b".repeat(40));
    kiwi_server::automation::follow::check(&k.kiwi).await;
    let state = phone.ok("/api/automation/reddit").await;
    assert!(state["follow"]["candidate"].is_null(), "{state}");
    assert!(
        state["follow"]["failed"].is_object(),
        "the given-up release stays shown"
    );

    // GitHub unreachable: shown, and nothing changes.
    gh.releases.lock().unwrap().take();
    *gh.releases.lock().unwrap() = json!({"message": "API rate limit exceeded"});
    kiwi_server::automation::follow::check(&k.kiwi).await;
    assert_eq!(
        phone.ok("/api/automation/reddit").await["tweak"],
        "RedditFilter 1.3.9"
    );

    // Turned off, builds use this Kiwi's own sources again.
    phone
        .send(
            Method::PATCH,
            "/api/settings",
            Some(json!({"follow_tweak_updates": false})),
        )
        .await;
    let off = phone.ok("/api/automation/reddit").await;
    assert_eq!(off["tweak"], "RedditFilter 1.3.8");
    assert_eq!(off["tweak_sources"][0]["sha"], RECIPE_SHA);
    phone.post("/api/automation/reddit/check", json!({})).await;
    let rebuilt = finished(&k).await;
    assert_eq!(rebuilt["published"]["version"], "2026.45.0", "{rebuilt}");
    assert!(last_pins(&k).is_null());
}

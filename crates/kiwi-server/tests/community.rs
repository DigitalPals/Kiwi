//! Community builds: the Morphe community list, its API and community jobs, with the
//! list, GitHub and the Android runner stood in for.
mod common;

use std::sync::{Arc, Mutex};

use axum::{
    Router,
    extract::State,
    http::{HeaderMap, StatusCode as Status, header},
    response::IntoResponse,
    routing::get,
};
use common::*;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

const DISCORD: &str = "community-com.discord";
const ANDROID_IP: &str = "100.64.0.9";

/// The community list and GitHub's API, as one local server.
#[derive(Default)]
struct Upstream {
    index: Mutex<Value>,
    /// Requests for the list that sent `If-None-Match`.
    conditional: Mutex<usize>,
    latest: Mutex<Value>,
}

async fn index(State(up): State<Arc<Upstream>>, headers: HeaderMap) -> impl IntoResponse {
    let body = up.index.lock().unwrap().to_string();
    let etag = format!("\"{}\"", body.len());
    if headers.get(header::IF_NONE_MATCH).is_some() {
        *up.conditional.lock().unwrap() += 1;
        if headers[header::IF_NONE_MATCH] == etag.as_str() {
            return (Status::NOT_MODIFIED, [(header::ETAG, etag)], String::new());
        }
    }
    (Status::OK, [(header::ETAG, etag)], body)
}

async fn latest(State(up): State<Arc<Upstream>>) -> impl IntoResponse {
    axum::Json(up.latest.lock().unwrap().clone())
}

async fn upstream() -> (Arc<Upstream>, String) {
    let up = Arc::new(Upstream::default());
    *up.index.lock().unwrap() = sample_index();
    let app = Router::new()
        .route("/data/bundles.json", get(index))
        .route("/repositories/42/releases/latest", get(latest))
        .with_state(up.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (up, base)
}

fn sample_index() -> Value {
    json!({
        "bundles": [
            {"source": "github", "repo": "VenusIsJaded/Venus-Patches", "author": "VenusIsJaded", "stars": 180,
             "updatedAt": 1793697356000_i64, "targetApps": ["com.discord"],
             "patches": [{"name": "Venus settings", "description": "Adds Venus to Discord's settings", "compatiblePackagesKey": 0},
                         {"name": "Keep deleted messages", "description": "Shows messages others deleted", "compatiblePackagesKey": 0}]},
            {"source": "github", "repo": "lildanlid1/Cordly", "author": "lildanlid1", "stars": 3,
             "updatedAt": 1793597356000_i64, "targetApps": ["com.discord"],
             "patches": [{"name": "Cordly settings", "compatiblePackagesKey": 0}]},
            {"source": "github", "repo": "browzomje/browzomje-patches", "author": "browzomje", "stars": 82,
             "updatedAt": 1793919255000_i64, "targetApps": ["com.pinterest"],
             "patches": [{"name": "Hide ads", "description": "Removes promoted pins", "compatiblePackagesKey": 1}]},
            {"source": "github", "repo": "someone/reddit", "author": "someone",
             "patches": [{"name": "Reddit thing", "compatiblePackagesKey": 2}]},
            {"source": "gitlab", "repo": "x/y", "patches": [{"name": "GitLab", "compatiblePackagesKey": 1}]}
        ],
        "store": {
            "com.discord": {"name": "Discord", "category": "Social", "installs": 700000000,
                            "iconUrl": "https://play-lh.googleusercontent.com/discord", "description": "Group chat"},
            "com.pinterest": {"name": "Pinterest", "installs": 900000000, "iconUrl": "https://play-lh.googleusercontent.com/pin"},
            "com.reddit.frontpage": {"name": "Reddit", "installs": 5}
        },
        "compatibilities": [
            [{"packageName": "com.discord", "targets": [{"version": "351.11 - Stable"}]}],
            [{"packageName": "com.pinterest", "targets": [{"version": "14.43.0"}]}],
            [{"packageName": "com.reddit.frontpage"}]
        ],
        "featured": ["com.discord", "com.reddit.frontpage"]
    })
}

/// Kiwi with Kiwi Builds on, the community list and GitHub served locally, a sandbox, and
/// a fake Android runner whose behaviour `<data>/fake-community.json` sets.
async fn community_kiwi() -> (TestKiwi, Arc<Upstream>) {
    let k = TestKiwi::start().await;
    let (up, base) = upstream().await;
    point_at(&k, &base);
    fake_runner(&k);
    let phone = k.phone();
    phone
        .send(
            Method::PATCH,
            "/api/settings",
            Some(json!({"kiwi_builds": true})),
        )
        .await;
    (k, up)
}

/// Points Kiwi's community list and GitHub at `base`, with a sandbox and no background work.
fn point_at(k: &TestKiwi, base: &str) {
    let c = &k.kiwi.community;
    c.background
        .store(false, std::sync::atomic::Ordering::SeqCst);
    *c.index_url.write().unwrap() = format!("{base}/data/bundles.json");
    *c.github_api.write().unwrap() = base.to_owned();
    c.set_sandbox(Ok(()));
}

fn behave(k: &TestKiwi, behaviour: Value) {
    std::fs::write(
        k.dir.path().join("data/fake-community.json"),
        behaviour.to_string(),
    )
    .unwrap();
}

fn last_request(k: &TestKiwi) -> Value {
    serde_json::from_slice(&std::fs::read(k.dir.path().join("data/last-request.json")).unwrap())
        .unwrap()
}

fn fake_runner(k: &TestKiwi) {
    let script = k.dir.path().join("fake-community-builder.py");
    let text = r#"
import sys,json,pathlib,hashlib,time
command=sys.argv[1]
data=pathlib.Path(sys.argv[sys.argv.index('--data-dir')+1])
root=data/'automation'
request=json.loads(pathlib.Path(sys.argv[sys.argv.index('--request')+1]).read_text())
def emit(event,/,**data):print(json.dumps(dict(event=event,**data)),flush=True)
assert command=='community', command
(data/'last-request.json').write_text(json.dumps(request))
b=data/'fake-community.json'
b=json.loads(b.read_text()) if b.exists() else {}
emit('stage',stage='Choosing patches')
time.sleep(b.get('pause',0))
cands=request['candidates']
for i,c in enumerate(cands[:2]):
    bad=b.get('ineligible') and i==0
    emit('community',kind='candidate',repo=c['repo'],repo_id=42,tag='v1.21.1',versions=['351.11 - Stable'],patches=2,default_patches=2,eligible=not bad,reason='no version Kiwi can download' if bad else None)
if b.get('fail'):
    emit('community',kind='failed',repo=cands[0]['repo'],tag='v1.21.1',reason='its patches failed (Stockfish analysis)')
    emit('error',message='No bundle built Discord.',blocked=False,kind=b.get('kind','other_bundle'))
    sys.exit(1)
emit('community',kind='certificate',sha256='e'*64)
chosen=cands[0]
tag=request.get('update_tag') or (request.get('pins') or {}).get('tag') or 'v1.21.1'
emit('stage',stage='Patching Discord')
blob=b'PK'+request['package'].encode()+str(request['sequence']).encode()+tag.encode()+b'x'*1000
sha=hashlib.sha256(blob).hexdigest()
(root/'artifacts').mkdir(exist_ok=True)
(root/'artifacts'/(sha+'.apk')).write_bytes(blob)
out=b.get('output',request['package'])
code=b.get('output_code',351011)
pins=dict(repo=chosen['repo'],repo_id=b.get('repo_id',42),author=chosen['repo'].split('/')[0],tag=tag,
    published_at='2026-11-01T00:00:00Z',asset='patches.mpp',asset_sha256=b.get('asset_sha256','a'*64),
    bundle_name=chosen['repo'].split('/')[1],version='351.11 - Stable',version_code=351011,output_package=out,
    output_version_code=code,patches=[dict(name='Venus settings',description='Adds Venus'),dict(name='Keep deleted messages',description='')])
emit('artifact',sha256=sha,version='351.11 - Stable',build=str(code),revision=request['sequence'],min_os=None,
    size=len(blob),package=out,signer='c'*64,notes='Fixture',community=pins)
"#;
    std::fs::write(&script, text).unwrap();
    *k.kiwi.automation.android_runner.write().unwrap() = script;
}

async fn finished(k: &TestKiwi, id: &str) -> Value {
    let phone = k.phone();
    for _ in 0..500 {
        let s = phone.ok(&format!("/api/automation/{id}")).await;
        if s["jobs"][0]["finished_at"].is_string() {
            return s;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("the community build did not finish");
}

async fn load_index(k: &TestKiwi) {
    kiwi_server::community::refresh(&k.kiwi, true)
        .await
        .unwrap();
}

#[tokio::test]
async fn the_community_list_is_loaded_shown_and_searched() {
    let (k, up) = community_kiwi().await;
    let phone = k.phone();

    // Before the first fetch: nothing to show, and a plain reason.
    let empty = phone.ok("/api/community").await;
    assert_eq!(empty["available"], false);
    assert_eq!(empty["index"], Value::Null);
    assert_eq!(empty["popular"], json!([]));
    assert!(
        empty["reason"].as_str().unwrap().contains("community list"),
        "{empty}"
    );

    load_index(&k).await;
    let overview = phone.ok("/api/community").await;
    assert_eq!(overview["available"], true, "{overview}");
    assert_eq!(overview["reason"], Value::Null);
    assert_eq!(overview["index"]["apps"], 2, "Reddit is Kiwi's own build");
    assert_eq!(overview["index"]["bundles"], 4, "GitLab is skipped");
    assert_eq!(overview["index"]["stale"], false);
    assert!(overview["index"]["updated_at"].is_string());
    assert_eq!(overview["added"], json!([]));
    let popular = overview["popular"].as_array().unwrap();
    let ids: Vec<&str> = popular.iter().map(|a| a["id"].as_str().unwrap()).collect();
    assert_eq!(
        ids,
        [DISCORD, "community-com.pinterest"],
        "featured first, then by installs"
    );
    let discord = &popular[0];
    assert_eq!(discord["package"], "com.discord");
    assert_eq!(discord["name"], "Discord");
    assert_eq!(discord["category"], "Social");
    assert_eq!(discord["installs"], 700000000);
    assert_eq!(discord["bundles"], 2);
    assert_eq!(discord["patches"], 2);
    assert_eq!(discord["featured"], true);
    assert_eq!(discord["added"], false);
    let icon = discord["icon_url"].as_str().unwrap();
    assert!(
        icon.starts_with("/api/art/"),
        "icons go through Kiwi: {icon}"
    );

    // Search: names, packages, patch names and descriptions.
    let search = |q: &str| {
        let phone = phone.clone();
        let q = q.to_owned();
        async move { phone.ok(&format!("/api/community/search?q={q}")).await }
    };
    let all = search("").await;
    assert_eq!(all["total"], 2);
    assert_eq!(all["results"][0]["package"], "com.pinterest", "by installs");
    assert_eq!(search("DISCORD").await["results"][0]["id"], DISCORD);
    assert_eq!(search("deleted%20messages").await["total"], 1);
    assert_eq!(
        search("promoted").await["results"][0]["package"],
        "com.pinterest"
    );
    assert_eq!(search("nothing").await["total"], 0);
    let page = phone.ok("/api/community/search?q=&offset=1&limit=1").await;
    assert_eq!(page["total"], 2);
    assert_eq!(page["results"].as_array().unwrap().len(), 1);
    assert_eq!(page["results"][0]["package"], "com.discord");
    let bad = phone.get("/api/community/search?limit=x").await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);

    // The list is fetched conditionally; an unusable one keeps the last good copy.
    load_index(&k).await;
    assert_eq!(*up.conditional.lock().unwrap(), 1);
    *up.index.lock().unwrap() = json!({"bundles": "hostile", "store": {}, "compatibilities": []});
    assert!(
        kiwi_server::community::refresh(&k.kiwi, true)
            .await
            .is_err()
    );
    let kept = phone.ok("/api/community").await;
    assert_eq!(kept["index"]["apps"], 2);
    assert_eq!(kept["index"]["stale"], true);
    assert_eq!(kept["available"], true);

    // The last good copy survives a restart.
    let dir = k.stop().await;
    let k = TestKiwi::start_in(dir, FakeDevices::with_phone(), false).await;
    point_at(&k, "http://127.0.0.1:9");
    let restarted = k.phone().ok("/api/community").await;
    assert_eq!(restarted["index"]["apps"], 2, "{restarted}");
}

#[tokio::test]
async fn community_builds_need_kiwi_builds_and_a_sandbox() {
    let (k, _up) = community_kiwi().await;
    load_index(&k).await;
    let phone = k.phone();
    k.kiwi
        .community
        .set_sandbox(Err(kiwi_server::community::NEEDS_LINUX.into()));
    let overview = phone.ok("/api/community").await;
    assert_eq!(overview["available"], false);
    assert_eq!(
        overview["reason"],
        "Community builds need Kiwi on Linux or in Docker, where Kiwi can lock the patcher in a sandbox."
    );
    assert_eq!(
        overview["popular"].as_array().unwrap().len(),
        2,
        "still browsable"
    );
    let refused = phone
        .post(&format!("/api/community/{DISCORD}/install"), json!({}))
        .await;
    assert_eq!(refused.status, StatusCode::CONFLICT);
    assert_eq!(refused.body["error"]["code"], "community_unavailable");

    phone
        .send(
            Method::PATCH,
            "/api/settings",
            Some(json!({"kiwi_builds": false})),
        )
        .await;
    for path in [
        "/api/community".to_owned(),
        "/api/community/search?q=x".to_owned(),
        format!("/api/community/{DISCORD}/candidates"),
    ] {
        let off = phone.get(&path).await;
        assert_eq!(off.status, StatusCode::CONFLICT, "{path}");
        assert_eq!(off.body["error"]["code"], "builds_off");
    }
}

#[tokio::test]
async fn installing_builds_once_publishes_and_installs_like_kiwis_own_builds() {
    let (k, _up) = community_kiwi().await;
    load_index(&k).await;
    k.devices.add_android(ANDROID_IP);
    let phone = k.phone();
    let android = k.android(ANDROID_IP);

    let unknown = phone
        .post(
            "/api/community/community-com.nothing.here/install",
            json!({}),
        )
        .await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    let reddit = phone
        .post(
            "/api/community/community-com.reddit.frontpage/install",
            json!({}),
        )
        .await;
    assert_eq!(
        reddit.status,
        StatusCode::NOT_FOUND,
        "Kiwi builds Reddit itself"
    );
    let hostile = phone
        .post("/api/community/community-..%2Fetc/install", json!({}))
        .await;
    assert_eq!(hostile.status, StatusCode::NOT_FOUND);
    let unlisted = phone
        .post(
            &format!("/api/community/{DISCORD}/install"),
            json!({"repo": "browzomje/browzomje-patches"}),
        )
        .await;
    assert_eq!(
        unlisted.status,
        StatusCode::BAD_REQUEST,
        "{}",
        unlisted.body
    );
    let malformed = phone
        .post(
            &format!("/api/community/{DISCORD}/install"),
            json!({"repo": "../../x"}),
        )
        .await;
    assert_eq!(malformed.status, StatusCode::BAD_REQUEST);
    let extra = phone
        .post(
            &format!("/api/community/{DISCORD}/install"),
            json!({"tweaks": []}),
        )
        .await;
    assert_eq!(extra.status, StatusCode::BAD_REQUEST);

    let installed = phone
        .post(&format!("/api/community/{DISCORD}/install"), json!({}))
        .await;
    assert_eq!(installed.status, StatusCode::ACCEPTED, "{}", installed.body);
    let state = &installed.body;
    assert_eq!(state["id"], DISCORD);
    assert_eq!(state["platform"], "android");
    assert_eq!(state["added"], true);
    assert_eq!(state["bundle_id"], "com.discord");
    assert_eq!(state["community"]["package"], "com.discord");
    assert_eq!(state["community"]["chosen"], "auto");
    assert_eq!(state["community"]["update"], Value::Null);
    assert_eq!(state["jobs"][0]["interactive"], true);

    let ready = finished(&k, DISCORD).await;
    assert_eq!(ready["jobs"][0]["status"], "ready", "{ready}");
    assert_eq!(ready["jobs"][0]["stage"], "Available in Kiwi");
    assert_eq!(ready["jobs"][0]["error_kind"], Value::Null);
    let request = last_request(&k);
    assert_eq!(request["mode"], "auto");
    assert_eq!(request["package"], "com.discord");
    let candidates: Vec<&str> = request["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["repo"].as_str().unwrap())
        .collect();
    assert_eq!(
        candidates,
        ["VenusIsJaded/Venus-Patches", "lildanlid1/Cordly"],
        "more default patches first"
    );
    assert_eq!(request["certificate"], Value::Null);
    let community = &ready["community"];
    assert_eq!(community["repo"], "VenusIsJaded/Venus-Patches");
    assert_eq!(community["author"], "VenusIsJaded");
    assert_eq!(community["tag"], "v1.21.1");
    assert_eq!(community["resolving"], false);
    assert_eq!(ready["tweak"], "Venus-Patches v1.21.1");
    assert_eq!(
        ready["summary"],
        "2 patches from VenusIsJaded/Venus-Patches"
    );
    assert_eq!(ready["app_version"], "351.11 - Stable");
    assert_eq!(ready["published"]["version"], "351.11 - Stable");
    assert_eq!(ready["included"][0]["name"], "Venus settings");
    assert!(
        ready["limitations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l["title"] == "Replaces the Play Store App")
    );
    let titles: Vec<&str> = ready["limitations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["title"].as_str().unwrap())
        .collect();
    assert!(titles.contains(&"Community · Not Reviewed"), "{titles:?}");

    // Listed among the added apps, and in the Kiwi Builds source for Android phones.
    let overview = phone.ok("/api/community").await;
    assert_eq!(overview["added"][0]["id"], DISCORD);
    assert_eq!(overview["popular"][0]["added"], true);
    let app = android.ok("/api/apps/kiwi-youtube/com.discord").await;
    assert_eq!(app["name"], "Discord");
    assert_eq!(app["state"]["action"], "install", "{app}");
    let prepared = android
        .post("/api/apps/kiwi-youtube/com.discord/prepare", json!({}))
        .await;
    assert_eq!(prepared.status, StatusCode::ACCEPTED, "{}", prepared.body);
    assert_eq!(prepared.body["status"], "ready");

    // Installing again changes nothing.
    let again = phone
        .post(&format!("/api/community/{DISCORD}/install"), json!({}))
        .await;
    assert_eq!(again.status, StatusCode::OK, "{}", again.body);
    let same = phone
        .post(
            &format!("/api/community/{DISCORD}/install"),
            json!({"repo": "venusisjaded/venus-patches"}),
        )
        .await;
    assert_eq!(same.status, StatusCode::OK);
    assert_eq!(same.body["jobs"].as_array().unwrap().len(), 1);

    // What Kiwi learned about the candidates, without network.
    let candidates = phone
        .ok(&format!("/api/community/{DISCORD}/candidates"))
        .await;
    let venus = &candidates[0];
    assert_eq!(venus["repo"], "VenusIsJaded/Venus-Patches");
    assert_eq!(venus["chosen"], true);
    assert_eq!(venus["recommended"], true);
    assert_eq!(venus["failed"], Value::Null);
    assert_eq!(venus["versions"], json!(["351.11 - Stable"]));
    assert_eq!(candidates[1]["chosen"], false);

    // No schedule or options for community apps.
    let path = format!("/api/automation/{DISCORD}");
    let patch = phone
        .send(Method::PATCH, &path, Some(ready["preferences"].clone()))
        .await;
    assert_eq!(patch.status, StatusCode::BAD_REQUEST);
    let check = phone.post(&format!("{path}/check"), json!({})).await;
    assert_eq!(check.status, StatusCode::BAD_REQUEST);

    // A manual choice builds the other bundle.
    let manual = phone
        .post(
            &format!("/api/community/{DISCORD}/install"),
            json!({"repo": "lildanlid1/Cordly"}),
        )
        .await;
    assert_eq!(manual.status, StatusCode::ACCEPTED);
    assert_eq!(manual.body["community"]["chosen"], "manual");
    assert_eq!(manual.body["community"]["repo"], "lildanlid1/Cordly");
    assert_eq!(manual.body["community"]["tag"], Value::Null);
    assert_eq!(manual.body["community"]["resolving"], true);
    assert_eq!(
        manual.body["tweak"], "Venus-Patches v1.21.1",
        "the published build until the new one is ready"
    );
    let rebuilt = finished(&k, DISCORD).await;
    assert_eq!(rebuilt["jobs"][0]["status"], "ready", "{rebuilt}");
    assert_eq!(last_request(&k)["mode"], "manual");
    assert_eq!(
        last_request(&k)["certificate"],
        "e".repeat(64),
        "pinned from the first build"
    );
    assert_eq!(rebuilt["community"]["repo"], "lildanlid1/Cordly");
    assert_eq!(rebuilt["community"]["chosen"], "manual");
    assert_eq!(rebuilt["published"]["revision"], 2);

    // Removing takes it out of the source and forgets the bundle.
    let removed = phone.send(Method::DELETE, &path, None).await;
    assert_eq!(removed.status, StatusCode::OK, "{}", removed.body);
    assert_eq!(removed.body["added"], false);
    assert_eq!(removed.body["community"]["tag"], Value::Null);
    assert_eq!(
        android
            .get("/api/apps/kiwi-youtube/com.discord")
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(phone.ok("/api/community").await["added"], json!([]));
}

#[tokio::test]
async fn pending_builds_are_joined_conflicts_refused_and_cancellable() {
    let (k, _up) = community_kiwi().await;
    load_index(&k).await;
    behave(&k, json!({"pause": 30}));
    let phone = k.phone();
    let path = format!("/api/community/{DISCORD}/install");
    let first = phone.post(&path, json!({})).await;
    assert_eq!(first.status, StatusCode::ACCEPTED);
    assert_eq!(first.body["community"]["resolving"], true);
    let joined = phone.post(&path, json!({})).await;
    assert_eq!(joined.status, StatusCode::ACCEPTED);
    assert_eq!(joined.body["jobs"].as_array().unwrap().len(), 1);
    let other = phone
        .post(&path, json!({"repo": "lildanlid1/Cordly"}))
        .await;
    assert_eq!(other.status, StatusCode::CONFLICT, "{}", other.body);
    // A second app waits behind it, with its place in line.
    let pinterest = phone
        .post("/api/community/community-com.pinterest/install", json!({}))
        .await;
    assert_eq!(pinterest.status, StatusCode::ACCEPTED);
    assert_eq!(pinterest.body["jobs"][0]["stage"], "Waiting for Discord");
    assert_eq!(pinterest.body["jobs"][0]["ahead"], 1);
    let cancel = phone
        .post("/api/automation/community-com.pinterest/cancel", json!({}))
        .await;
    assert_eq!(cancel.status, StatusCode::OK);
    assert_eq!(cancel.body["jobs"][0]["status"], "cancelled");
    let cancel = phone
        .post(&format!("/api/automation/{DISCORD}/cancel"), json!({}))
        .await;
    assert_eq!(cancel.status, StatusCode::OK);
    let done = finished(&k, DISCORD).await;
    assert_eq!(done["jobs"][0]["status"], "cancelled", "{done}");
    assert_eq!(done["jobs"][0]["error_kind"], Value::Null);
    let unknown = phone
        .post("/api/automation/community-com.nothing/cancel", json!({}))
        .await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn failures_carry_what_the_owner_can_do_and_mark_the_bundle() {
    let (k, _up) = community_kiwi().await;
    load_index(&k).await;
    behave(&k, json!({"fail": true}));
    let phone = k.phone();
    phone
        .post(&format!("/api/community/{DISCORD}/install"), json!({}))
        .await;
    let failed = finished(&k, DISCORD).await;
    let job = &failed["jobs"][0];
    assert_eq!(job["status"], "failed", "{failed}");
    assert_eq!(job["error"], "No bundle built Discord.");
    assert_eq!(job["error_kind"], "other_bundle");
    let candidates = phone
        .ok(&format!("/api/community/{DISCORD}/candidates"))
        .await;
    assert_eq!(
        candidates[0]["failed"],
        "its patches failed (Stockfish analysis)"
    );
    assert_eq!(candidates[0]["recommended"], false);
    assert_eq!(candidates[1]["recommended"], true);

    // The next automatic build skips the failed bundle while that release is its latest.
    behave(&k, json!({}));
    phone
        .post(&format!("/api/community/{DISCORD}/install"), json!({}))
        .await;
    let ready = finished(&k, DISCORD).await;
    assert_eq!(ready["jobs"][0]["status"], "ready", "{ready}");
    assert_eq!(
        last_request(&k)["skip"],
        json!({"VenusIsJaded/Venus-Patches": "v1.21.1"})
    );

    // A bundle that wasn't eligible is shown with its reason but checked again next time.
    let path = format!("/api/automation/{DISCORD}");
    phone.send(Method::DELETE, &path, None).await;
    behave(&k, json!({"ineligible": true}));
    phone
        .post(&format!("/api/community/{DISCORD}/install"), json!({}))
        .await;
    assert_eq!(finished(&k, DISCORD).await["jobs"][0]["status"], "ready");
    let candidates = phone
        .ok(&format!("/api/community/{DISCORD}/candidates"))
        .await;
    // (The fake runner reports the first candidate ineligible and builds it anyway.)
    let venus = candidates
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["repo"] == "VenusIsJaded/Venus-Patches")
        .unwrap();
    assert_eq!(
        venus["failed"], "no version Kiwi can download",
        "{candidates}"
    );
    phone.send(Method::DELETE, &path, None).await;
    behave(&k, json!({}));
    phone
        .post(&format!("/api/community/{DISCORD}/install"), json!({}))
        .await;
    finished(&k, DISCORD).await;
    let skip = last_request(&k)["skip"].clone();
    assert!(skip.get("VenusIsJaded/Venus-Patches").is_none(), "{skip}");

    // A retry-type failure.
    behave(&k, json!({"fail": true, "kind": "retry"}));
    phone
        .post(
            &format!("/api/community/{DISCORD}/install"),
            json!({"repo": "lildanlid1/Cordly"}),
        )
        .await;
    assert_eq!(
        finished(&k, DISCORD).await["jobs"][0]["error_kind"],
        "retry"
    );
}

#[tokio::test]
async fn a_build_that_breaks_its_pins_or_kiwis_own_apps_is_refused() {
    let (k, _up) = community_kiwi().await;
    load_index(&k).await;
    let phone = k.phone();
    // A bundle that renames Discord to Kiwi's own YouTube build.
    behave(&k, json!({"output": "app.morphe.android.youtube"}));
    phone
        .post(&format!("/api/community/{DISCORD}/install"), json!({}))
        .await;
    let refused = finished(&k, DISCORD).await;
    assert_eq!(refused["jobs"][0]["status"], "blocked", "{refused}");
    assert_eq!(refused["jobs"][0]["error_kind"], "other_bundle");
    assert_eq!(refused["published"], Value::Null);
    assert_eq!(refused["community"]["tag"], Value::Null, "nothing pinned");
}

#[tokio::test]
async fn updates_are_offered_after_three_days_and_built_when_approved() {
    let (k, up) = community_kiwi().await;
    let base = k.kiwi.community.github_api.read().unwrap().clone();
    load_index(&k).await;
    let phone = k.phone();
    phone
        .post(&format!("/api/community/{DISCORD}/install"), json!({}))
        .await;
    assert_eq!(finished(&k, DISCORD).await["jobs"][0]["status"], "ready");
    let path = format!("/api/community/{DISCORD}/update");
    let none = phone.post(&path, json!({})).await;
    assert_eq!(none.status, StatusCode::CONFLICT, "no update offered");

    let release = |tag: &str, days: i64| {
        json!({"tag_name": tag, "draft": false, "prerelease": false,
            "published_at": (chrono::Utc::now() - chrono::Duration::days(days)).to_rfc3339(),
            "assets": [{"id": 7, "name": "patches.mpp", "size": 1000}]})
    };
    // A release from yesterday is not offered yet, and nothing builds by itself.
    *up.latest.lock().unwrap() = release("v1.22.0", 1);
    kiwi_server::community::check_updates(&k.kiwi).await;
    let state = phone.ok(&format!("/api/automation/{DISCORD}")).await;
    assert_eq!(state["community"]["update"], Value::Null);
    // Checked daily: it waits for tomorrow.
    *up.latest.lock().unwrap() = release("v1.22.0", 4);
    kiwi_server::community::check_updates(&k.kiwi).await;
    assert_eq!(
        phone.ok(&format!("/api/automation/{DISCORD}")).await["community"]["update"],
        Value::Null
    );
    // As if a day passed: the check is due after a restart with an old check time.
    let mut saved: Value =
        serde_json::from_str(&k.kiwi.db.meta("automation_v2").unwrap().unwrap()).unwrap();
    saved["community"][DISCORD]["update_checked_at"] = json!("2026-01-01T00:00:00Z");
    k.kiwi
        .db
        .set_meta("automation_v2", &saved.to_string())
        .unwrap();
    let dir = k.stop().await;
    let k = TestKiwi::start_in(dir, FakeDevices::with_phone(), false).await;
    point_at(&k, &base);
    fake_runner(&k);
    kiwi_server::community::check_updates(&k.kiwi).await;
    let phone = k.phone();
    let offered = phone.ok(&format!("/api/automation/{DISCORD}")).await;
    assert_eq!(
        offered["community"]["update"]["tag"], "v1.22.0",
        "{offered}"
    );
    assert_eq!(
        offered["jobs"].as_array().unwrap().len(),
        1,
        "never built by itself"
    );

    let approved = phone.post(&path, json!({})).await;
    assert_eq!(approved.status, StatusCode::ACCEPTED, "{}", approved.body);
    let updated = finished(&k, DISCORD).await;
    assert_eq!(updated["jobs"][0]["status"], "ready", "{updated}");
    let request = last_request(&k);
    assert_eq!(request["mode"], "update");
    assert_eq!(request["update_tag"], "v1.22.0");
    assert_eq!(request["pins"]["tag"], "v1.21.1");
    assert_eq!(updated["community"]["tag"], "v1.22.0");
    assert_eq!(updated["community"]["update"], Value::Null);
    assert_eq!(updated["tweak"], "Venus-Patches v1.22.0");
}

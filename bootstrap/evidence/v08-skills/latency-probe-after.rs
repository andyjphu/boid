// Disposable measurement module, not part of the product test suite.
use super::*;
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skill_snapshot_latency_observations() {
    use axum::{Router, Json, routing::post};
    use std::time::{Instant, Duration};
    let (sent, mut requested) = tokio::sync::mpsc::unbounded_channel::<()>();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/",listener.local_addr().unwrap());
    let router = Router::new().route("/",post(move |Json(_request): Json<Value>| {
        let sent=sent.clone();async move {
            sent.send(()).unwrap();
            std::future::pending::<()>().await;
            "unreachable"
        }
    }));
    let server=tokio::spawn(async move {axum::serve(listener,router).await.unwrap()});
    let root=std::path::Path::new("/work/checks/latency-state");
    std::fs::create_dir_all(root).unwrap();
    let mut samples=vec![];
    for mode in ["empty", "humanizer", "bank"] {
        for count in [0,100] {
            let state=tempfile::tempdir_in(root).unwrap();
            let work=tempfile::tempdir_in(root).unwrap();
            let cwd=if mode=="bank" {PathBuf::from("/work/checks/skill-bank")} else {work.path().into()};
            let start=Instant::now();
            let mut snapshot=crate::skills::discovery::scan(&cwd);
            if mode=="empty" {snapshot.skills.clear();}
            let scan_ms=start.elapsed().as_secs_f64()*1000.;
            let skill_count=snapshot.skills.len();
            let snapshot_bytes=serde_json::to_vec(&snapshot.skills).unwrap().len();
            let mut engine=Session::new("test".into(),crate::skills::instructions(&snapshot.skills),tool_calls::definitions(),"Isolated measurement".into());
            engine.skills=snapshot.skills;
            if mode!="empty" {engine.skill_base=Some(include_str!("PRINCIPLES.md").into());}
            let mut entries=vec![Hub::make_entry("context",engine.instructions.clone())];
            for i in 0..count {
                let text=format!("Synthetic retained turn {i}: {}", "x".repeat(1024));
                engine.user(text.clone());entries.push(Hub::make_entry("user",text));
            }
            let hub=Hub::open(Client::test_client_at(endpoint.clone()),state.path().into()).unwrap();
            let id=Uuid::new_v4().to_string();
            let mut record=Record {version:1,view:View{id:id.clone(),name:mode.into(),profile:json!({}),cwd:cwd.to_string_lossy().into_owned(),status:"idle".into(),working_since_ms:None,entries,pending:vec![],archived:false},engine,inputs:VecDeque::new(),inbox:VecDeque::new(),order:0,stop_through:0,paused:false,stream_seq:0};
            let record_bytes=serde_json::to_vec(&record).unwrap().len();
            hub.save_record(&id,&mut record).unwrap();hub.install(record,true).unwrap();
            let start=Instant::now();hub.message(&id,"Measured input".into()).unwrap();
            let enter_ms=start.elapsed().as_secs_f64()*1000.;
            tokio::time::timeout(Duration::from_secs(10),requested.recv()).await.unwrap().unwrap();
            let start=Instant::now();hub.interrupt(&id).unwrap();
            let interrupt_accept_ms=start.elapsed().as_secs_f64()*1000.;
            tokio::time::timeout(Duration::from_secs(10),async {while hub.view(&id).unwrap().status!="stopped" {tokio::task::yield_now().await;}}).await.unwrap();
            let interrupt_stopped_ms=start.elapsed().as_secs_f64()*1000.;
            samples.push(json!({"mode":mode,"history_entries":count,"skill_count":skill_count,"snapshot_bytes":snapshot_bytes,"record_bytes":record_bytes,"scan_ms":scan_ms,"enter_ms":enter_ms,"interrupt_accept_ms":interrupt_accept_ms,"interrupt_stopped_ms":interrupt_stopped_ms}));
            hub.shutdown().await;
        }
    }
    server.abort();
    let result=json!({"conditions":"Same debug test binary; two Tokio workers; Docker bind mount; gated local mock provider; synthetic retained turns of 1024 padding characters; one observation per condition, not percentiles. Empty condition clears skill snapshot and disables refresh, not a separate main binary.","samples":samples});
    std::fs::write("/work/checks/latency-results.json",serde_json::to_vec_pretty(&result).unwrap()).unwrap();
    println!("{result}");
}

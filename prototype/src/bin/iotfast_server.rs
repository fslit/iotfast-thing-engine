use data_encoding::BASE64;
use iotfast_thing_engine::config::EngineConfig;
use iotfast_thing_engine::scope::VerifiedScopeContext;
use iotfast_thing_engine::server::{IoStream, ServerHandle};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::env;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

fn main() {
    let mut listen_addr =
        env::var("IOTFAST_LISTEN_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".into());
    let mut data_dir = env::var("IOTFAST_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("./target/iotfast-data"));
    let mut auth_secret = env::var("IOTFAST_AUTH_HMAC_SECRET")
        .unwrap_or_else(|_| "iotfast-default-dev-hmac-secret-2026".into());
    let mut issue_token_json: Option<String> = None;
    let mut stdio_stream = false;

    let args: Vec<String> = env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--listen" if i + 1 < args.len() => {
                listen_addr = args[i + 1].clone();
                i += 2;
            }
            "--data-dir" if i + 1 < args.len() => {
                data_dir = PathBuf::from(&args[i + 1]);
                i += 2;
            }
            "--auth-secret" if i + 1 < args.len() => {
                auth_secret = args[i + 1].clone();
                i += 2;
            }
            "--issue-token-json" if i + 1 < args.len() => {
                issue_token_json = Some(args[i + 1].clone());
                i += 2;
            }
            "--stdio-stream" => {
                stdio_stream = true;
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }

    if let Some(raw_json) = issue_token_json {
        let ctx: VerifiedScopeContext =
            serde_json::from_str(&raw_json).expect("valid VerifiedScopeContext JSON");
        println!("{}", ctx.issue_signed_token(&auth_secret));
        return;
    }

    let cfg = EngineConfig::for_production(&data_dir);
    cfg.validate().expect("Invalid EngineConfig");
    let mut handle = ServerHandle::spawn_on(&listen_addr, cfg, auth_secret)
        .expect("Failed to start iotfast_server");
    println!(
        "{{\"status\":\"listening\",\"addr\":\"{}\",\"base_url\":\"{}\",\"ws_url\":\"{}\"}}",
        handle.addr(),
        handle.base_url(),
        handle.ws_url()
    );
    let _ = std::io::stdout().flush();

    if stdio_stream {
        run_stdio_stream_mux(&mut handle);
        return;
    }

    while !handle.context().shutdown.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(200));
    }
}

fn run_stdio_stream_mux(handle: &mut ServerHandle) {
    let stdin = std::io::stdin();
    let mut streams: BTreeMap<u64, Box<dyn IoStream>> = BTreeMap::new();
    for line in BufReader::new(stdin.lock()).lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(cmd) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let op = cmd["op"].as_str().unwrap_or("");
        let cid = cmd["cid"].as_u64().unwrap_or(0);
        match op {
            "http" => {
                let raw_req = BASE64
                    .decode(cmd["req_b64"].as_str().unwrap_or("").as_bytes())
                    .unwrap_or_default();
                let mut s = handle.connect();
                let _ = s.write_all(&raw_req);
                let _ = s.flush();
                let mut resp = Vec::new();
                let _ = s.read_to_end(&mut resp);
                println!(
                    "{}",
                    json!({ "cid": cid, "resp_b64": BASE64.encode(&resp) })
                );
                let _ = std::io::stdout().flush();
            }
            "ws_open" => {
                let raw_req = BASE64
                    .decode(cmd["req_b64"].as_str().unwrap_or("").as_bytes())
                    .unwrap_or_default();
                let mut s = handle.connect();
                let _ = s.write_all(&raw_req);
                let _ = s.flush();
                let mut header_buf = Vec::new();
                let mut byte = [0u8; 1];
                while !header_buf.ends_with(b"\r\n\r\n") {
                    if s.read_exact(&mut byte).is_err() {
                        break;
                    }
                    header_buf.push(byte[0]);
                }
                streams.insert(cid, s);
                println!(
                    "{}",
                    json!({ "cid": cid, "resp_b64": BASE64.encode(&header_buf) })
                );
                let _ = std::io::stdout().flush();
            }
            "ws_send" => {
                if let Some(s) = streams.get_mut(&cid) {
                    let raw = BASE64
                        .decode(cmd["frame_b64"].as_str().unwrap_or("").as_bytes())
                        .unwrap_or_default();
                    let _ = s.write_all(&raw);
                    let _ = s.flush();
                }
                println!("{}", json!({ "cid": cid, "ok": true }));
                let _ = std::io::stdout().flush();
            }
            "ws_recv" => {
                let frame = if let Some(s) = streams.get_mut(&cid) {
                    iotfast_thing_engine::server::read_ws_text_frame(s)
                        .ok()
                        .flatten()
                } else {
                    None
                };
                println!("{}", json!({ "cid": cid, "text": frame }));
                let _ = std::io::stdout().flush();
            }
            "ws_close" => {
                streams.remove(&cid);
                println!("{}", json!({ "cid": cid, "closed": true }));
                let _ = std::io::stdout().flush();
            }
            "shutdown" => {
                handle.shutdown();
                break;
            }
            _ => {}
        }
    }
}

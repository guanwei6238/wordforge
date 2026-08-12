//! 對著真實資料庫與真實後端出一題，量各階段耗時。
//!
//! 會消耗訂閱額度，所以預設 ignore：
//!
//! ```bash
//! cargo test --release -p wordforge-practice --test live -- --ignored --nocapture
//! ```
use std::time::Instant;
use time::OffsetDateTime;
use wordforge_core::practice::ExerciseKind;
use wordforge_db::Db;
use wordforge_practice::PracticeEngine;

#[tokio::test]
#[ignore = "會呼叫真的模型，消耗訂閱額度"]
async fn time_a_reading_exercise() {
    tracing_subscriber::fmt().with_env_filter("info").init();

    let db_path = std::env::var("WORDFORGE_DB").expect("設 WORDFORGE_DB");
    let open = Instant::now();
    let db = Db::open(std::path::Path::new(&db_path)).await.unwrap();
    println!("開資料庫（含 migration）: {:?}", open.elapsed());

    let cfg = wordforge_llm::CliConfig {
        preset: wordforge_llm::CliPreset::Codex,
        program: "codex".into(),
        args: vec!["exec".into(), "--skip-git-repo-check".into()],
        system_flag: None,
        model_flag: Some("-m".into()),
        model: std::env::var("MODEL").unwrap_or_else(|_| "gpt-5.6-luna".into()),
        effort_style: wordforge_llm::EffortStyle::Config {
            flag: "-c".into(),
            key: "model_reasoning_effort".into(),
        },
        effort: std::env::var("EFFORT").unwrap_or_else(|_| "low".into()),
        timeout_secs: 300,
    };
    println!("model={} effort={}", cfg.model, cfg.effort);
    let llm = wordforge_llm::CliLlm::new(cfg).unwrap();

    let build = Instant::now();
    let engine = PracticeEngine::for_profile(&db, &llm, 1).await.unwrap();
    println!("建引擎（讀 profile 語言）: {:?}", build.elapsed());

    let total = Instant::now();
    let result = engine
        .generate(1, Some(ExerciseKind::Reading), OffsetDateTime::now_utc())
        .await;
    println!("\n=== 整題總耗時: {:?} ===", total.elapsed());

    match result {
        Ok(ex) => println!(
            "成功：coverage={:?} 目標字={:?}",
            ex.coverage, ex.target_words
        ),
        Err(e) => println!("失敗：{e}"),
    }
}

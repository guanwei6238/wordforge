//! Wordforge 命令列工具。
//!
//! 首次載入一份完整字典要處理幾 GB、上百萬筆資料，跑在 GUI 裡會綁住視窗
//! 好幾十分鐘；用 CLI 匯入完再打開 App 是比較舒服的做法。
//! 這支工具寫的是**同一個資料庫檔案**，所以匯入完 App 立刻就看得到。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use time::OffsetDateTime;
use wordforge_core::model::{CardKind, ProfileId};
use wordforge_db::{Db, repo};
use wordforge_import::{FreqFormat, ImportOptions, ImportProgress, NoProgress, ProgressSink};

/// 必須跟 `tauri.conf.json` 的 identifier 一致，否則 CLI 和 App 會各寫各的資料庫。
const APP_IDENTIFIER: &str = "org.wordforge.app";

/// App 首次啟動時建立的預設 profile。CLI 目前只操作這一個。
const DEFAULT_PROFILE: i64 = 1;

/// 標籤的中文說明，只用於 CLI 輸出。
const TAG_NAMES: [(&str, &str); 9] = [
    ("zk", "國中會考"),
    ("gk", "學測"),
    ("cet4", "大學英語四級"),
    ("cet6", "大學英語六級"),
    ("ky", "考研"),
    ("toefl", "托福"),
    ("ielts", "雅思"),
    ("gre", "GRE"),
    ("oxford3000", "牛津核心三千"),
];

#[derive(Parser)]
#[command(name = "wordforge", version, about = "Wordforge 命令列工具")]
struct Cli {
    /// 資料庫路徑。預設用桌面 App 的同一個檔案。
    #[arg(long, global = true)]
    db: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 顯示資料庫路徑
    Path,
    /// 顯示字典規模與來源
    Stats,
    /// 查一個字
    Search {
        query: String,
        #[arg(long, default_value = "en")]
        lang: String,
        #[arg(long, default_value_t = 10)]
        limit: i64,
    },
    /// 匯入字典或詞頻表
    #[command(subcommand)]
    Import(ImportCmd),
    /// 管理牌組
    #[command(subcommand)]
    Deck(DeckCmd),
}

#[derive(Subcommand)]
enum DeckCmd {
    /// 列出可用的標籤與各自的字數
    Tags {
        #[arg(long, default_value = "en")]
        lang: String,
    },
    /// 依標籤批次加入單字，由常用到罕見
    Add {
        /// 標籤，如 zk（國中會考）、gk（學測）、cet4、ielts
        #[arg(long)]
        tag: String,
        #[arg(long, default_value = "en")]
        lang: String,
        /// 最多加入幾個字
        #[arg(long, default_value_t = 500)]
        limit: i64,
        /// 卡片類型，可重複指定：recognition / recall / listening / spelling
        #[arg(long = "kind", default_values_t = [String::from("recognition")])]
        kinds: Vec<String>,
        /// 連 the / of / and 這類功能詞也一起加入（預設排除，它們該從閱讀中學）
        #[arg(long)]
        include_function_words: bool,
        /// 跳過比這個詞頻排名更常用的字（已經會的就不用再排）
        #[arg(long, default_value_t = 0)]
        from_rank: i64,
    },
}

#[derive(Subcommand)]
enum ImportCmd {
    /// ECDICT 英漢字典（含中文翻譯、音標、詞形、考試標籤）
    Ecdict { path: PathBuf },
    /// kaikki.org 的 Wiktionary JSONL
    Wiktionary {
        path: PathBuf,
        #[arg(long, default_value = "en")]
        lang: String,
    },
    /// 通用 CSV / TSV 單字表
    Csv {
        path: PathBuf,
        #[arg(long, default_value = "en")]
        lang: String,
        /// 顯示在來源清單上的名稱
        #[arg(long, default_value = "我的單字表")]
        name: String,
        /// 用 Tab 分隔
        #[arg(long)]
        tsv: bool,
    },
    /// 詞頻表：只更新既有詞條的排名，不會新增詞條
    Freq {
        path: PathBuf,
        #[arg(long, default_value = "en")]
        lang: String,
        #[arg(long, value_enum, default_value_t = FreqKind::Ranked)]
        format: FreqKind,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum FreqKind {
    /// 一行一個字，行號即排名
    Ranked,
    /// `字<TAB>次數`
    Tab,
    /// `字,次數`
    Comma,
    /// `字 次數`
    Space,
}

impl From<FreqKind> for FreqFormat {
    fn from(k: FreqKind) -> Self {
        match k {
            FreqKind::Ranked => FreqFormat::RankedList,
            FreqKind::Tab => FreqFormat::TabCounts,
            FreqKind::Comma => FreqFormat::CommaCounts,
            FreqKind::Space => FreqFormat::SpaceCounts,
        }
    }
}

/// 與 Tauri v2 `app_data_dir()` 對齊的路徑推導。
///
/// 刻意手寫而不引入 `dirs`：規則只有三條，而「跟 App 用同一個檔案」
/// 是這支工具的全部意義，值得看得見。
fn default_db_path() -> Result<PathBuf> {
    let base = if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .context("找不到 %APPDATA%")?
    } else {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .context("找不到 $HOME")?;
        if cfg!(target_os = "macos") {
            home.join("Library/Application Support")
        } else {
            std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".local/share"))
        }
    };
    Ok(base.join(APP_IDENTIFIER).join("wordforge.db"))
}

/// 印進度到 stderr，用 `\r` 原地更新，不洗版。
struct ConsoleProgress {
    last_len: AtomicU64,
}

impl ConsoleProgress {
    fn new() -> Self {
        Self {
            last_len: AtomicU64::new(0),
        }
    }
}

impl ProgressSink for ConsoleProgress {
    fn report(&self, p: &ImportProgress) {
        let pct = p
            .fraction()
            .map(|f| format!("{:>5.1}%  ", f * 100.0))
            .unwrap_or_default();
        let line = format!(
            "\r{pct}已處理 {:>9} · 匯入 {:>9} · 跳過 {:>7} · 失敗 {:>5}",
            p.processed, p.imported, p.skipped, p.failed
        );
        // 上一行比較長的話要補空白蓋掉殘影
        let pad = (self.last_len.load(Ordering::Relaxed) as usize).saturating_sub(line.len());
        self.last_len.store(line.len() as u64, Ordering::Relaxed);
        eprint!("{line}{}", " ".repeat(pad));
        let _ = std::io::stderr().flush();
    }
}

async fn open_db(path: &Path) -> Result<Db> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("建立資料夾失敗：{}", dir.display()))?;
    }
    Db::open(path)
        .await
        .with_context(|| format!("開啟資料庫失敗：{}", path.display()))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,sqlx::query=error".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let db_path = match cli.db {
        Some(p) => p,
        None => default_db_path()?,
    };

    if matches!(cli.command, Command::Path) {
        println!("{}", db_path.display());
        return Ok(());
    }

    let db = open_db(&db_path).await?;
    let opts = ImportOptions::default();

    match cli.command {
        Command::Path => unreachable!("上面已經處理掉了"),

        Command::Stats => {
            let s = wordforge_db::dict::stats(&db).await?;
            println!(
                "詞條 {}　釋義 {}　有音檔 {}",
                s.lemmas, s.senses, s.with_audio
            );
            if s.sources.is_empty() {
                println!("\n還沒有匯入任何字典。");
            } else {
                println!("\n來源：");
                for src in s.sources {
                    println!(
                        "  {:<28} {:>8} 筆　{}",
                        src.name,
                        src.lemma_count,
                        src.license.as_deref().unwrap_or("授權未標示")
                    );
                }
            }
        }

        Command::Search { query, lang, limit } => {
            let hits =
                wordforge_db::dict::search(&db, &lang, &query, DEFAULT_PROFILE, limit).await?;
            if hits.is_empty() {
                println!("查不到「{query}」");
            }
            for h in hits {
                let tags = if h.tags.is_empty() {
                    String::new()
                } else {
                    format!("  [{}]", h.tags.join(" "))
                };
                println!(
                    "{:<20} {:<6} {}{}",
                    h.text,
                    h.pos,
                    h.translation.or(h.gloss).unwrap_or_default(),
                    tags
                );
            }
        }

        Command::Deck(DeckCmd::Tags { lang }) => {
            let summary =
                repo::cards::tag_summary(&db, ProfileId(DEFAULT_PROFILE), &lang, 0).await?;
            if summary.is_empty() {
                println!("字典裡沒有任何標籤。ECDICT 才有考試範圍標籤。");
            }
            for (tag, total, in_deck) in summary {
                println!(
                    "  {:<12} {:>6} 字　已加入 {:>6}　{}",
                    tag,
                    total,
                    in_deck,
                    TAG_NAMES
                        .iter()
                        .find(|(k, _)| *k == tag)
                        .map(|(_, v)| *v)
                        .unwrap_or("")
                );
            }
        }

        Command::Deck(DeckCmd::Add {
            tag,
            lang,
            limit,
            kinds,
            include_function_words,
            from_rank,
        }) => {
            let kinds: Vec<CardKind> = kinds
                .iter()
                .map(|k| match k.as_str() {
                    "recognition" => Ok(CardKind::Recognition),
                    "recall" => Ok(CardKind::Recall),
                    "listening" => Ok(CardKind::Listening),
                    "spelling" => Ok(CardKind::Spelling),
                    other => Err(anyhow::anyhow!("未知的卡片類型：{other}")),
                })
                .collect::<Result<_>>()?;

            let added = repo::cards::add_by_tag(
                &db,
                ProfileId(DEFAULT_PROFILE),
                repo::cards::AddByTag {
                    lang: &lang,
                    tag: &tag,
                    kinds: &kinds,
                    limit,
                    skip_function_words: !include_function_words,
                    min_freq_rank: from_rank,
                },
                OffsetDateTime::now_utc(),
            )
            .await?;
            println!("加入 {added} 張卡片（標籤 {tag}，上限 {limit} 字）");
        }

        Command::Import(cmd) => {
            let progress = ConsoleProgress::new();
            let started = std::time::Instant::now();

            let result = match cmd {
                ImportCmd::Ecdict { path } => {
                    let meta = wordforge_dict::ecdict::source_meta();
                    let source = wordforge_import::register_source(&db, &meta).await?;
                    let file = std::fs::File::open(&path)
                        .with_context(|| format!("開啟失敗：{}", path.display()))?;
                    let total = file.metadata().map(|m| m.len()).unwrap_or(0);
                    let reader = std::io::BufReader::with_capacity(1 << 20, file);

                    let mut p = wordforge_import::import_entries(
                        &db,
                        source,
                        wordforge_dict::ecdict::parse(reader),
                        &opts,
                        &progress,
                    )
                    .await?;
                    p.bytes_total = total;
                    p
                }

                ImportCmd::Wiktionary { path, lang } => {
                    wordforge_import::import_wiktionary_jsonl(&db, &path, &lang, &opts, &progress)
                        .await?
                }

                ImportCmd::Csv {
                    path,
                    lang,
                    name,
                    tsv,
                } => {
                    let delimiter = if tsv { b'\t' } else { b',' };
                    wordforge_import::import_csv(
                        &db, &path, &lang, delimiter, &name, &opts, &progress,
                    )
                    .await?
                }

                ImportCmd::Freq { path, lang, format } => {
                    let updated =
                        wordforge_import::import_freq_list(&db, &path, &lang, format.into())
                            .await?;
                    eprintln!();
                    println!(
                        "更新了 {updated} 個詞條的詞頻排名，耗時 {:.1?}",
                        started.elapsed()
                    );
                    return Ok(());
                }
            };

            eprintln!();
            println!(
                "匯入 {} 筆，跳過 {} 筆，失敗 {} 筆，耗時 {:.1?}",
                result.imported,
                result.skipped,
                result.failed,
                started.elapsed()
            );

            // 大量寫入後重建統計資訊，之後的查詢才會選對索引
            eprintln!("整理資料庫…");
            db.optimize().await?;
        }
    }

    Ok(())
}

/// 讓 `NoProgress` 不會因為只在測試用到而被判定成 dead code。
#[allow(dead_code)]
fn _assert_no_progress_is_usable() -> impl ProgressSink {
    NoProgress
}

//! # wordforge-import
//!
//! 把 [`wordforge_dict`] 解析出來的詞條批次寫進資料庫。
//!
//! 這一層獨立存在的理由：字典 dump 動輒數 GB、上百萬筆，匯入不是
//! 「跑一個 for 迴圈」那麼單純，還要處理
//!
//! - **批次 transaction**：每筆各自 commit 的話，一百萬筆要跑好幾個小時
//! - **進度回報**：使用者需要知道還要等多久，且不能每筆都發事件淹沒 UI
//! - **中斷**：按下取消要在合理時間內停下，且已匯入的部分要保留
//! - **容錯**：幾百萬行裡有幾行壞掉是常態，不該讓整批失敗
//!
//! `wordforge-dict` 只負責解析、`wordforge-db` 只負責 SQL，這些流程控制
//! 放在兩者之間。

pub mod audio;
pub mod material;

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;
use time::OffsetDateTime;
use wordforge_db::Db;
use wordforge_db::dict::{
    self, EntryWrite, NewExample, NewPronunciation, NewSense, NewSource, SourceId,
};
use wordforge_dict::{DictEntry, DictError, SourceMeta};

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error(transparent)]
    Db(#[from] wordforge_db::DbError),

    #[error(transparent)]
    Dict(#[from] DictError),

    #[error("讀取檔案失敗：{0}")]
    Io(#[from] std::io::Error),

    #[error("資料庫操作失敗：{0}")]
    Sqlx(#[from] sqlx::Error),

    #[error("{0}")]
    Parse(String),
}

pub type Result<T> = std::result::Result<T, ImportError>;

/// 匯入進度。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ImportProgress {
    /// 讀到的資料筆數（含跳過與失敗的）
    pub processed: u64,
    /// 實際寫進資料庫的詞條數
    pub imported: u64,
    /// 沒有任何釋義而跳過的（重定向頁、格式雜訊）
    pub skipped: u64,
    /// 解析失敗的行
    pub failed: u64,
    /// 已讀位元組數，用於算百分比
    pub bytes_read: u64,
    /// 檔案總位元組數；未知時為 0
    pub bytes_total: u64,
    /// 是否因為使用者取消而提早結束
    pub cancelled: bool,
}

impl ImportProgress {
    /// 完成度 0.0~1.0。無法得知檔案大小時回傳 `None`。
    pub fn fraction(&self) -> Option<f64> {
        (self.bytes_total > 0)
            .then(|| (self.bytes_read as f64 / self.bytes_total as f64).clamp(0.0, 1.0))
    }
}

/// 進度回報與取消的介面。
///
/// UI 端實作它來發 Tauri 事件；測試與 CLI 用 [`NoProgress`]。
pub trait ProgressSink: Send + Sync {
    fn report(&self, progress: &ImportProgress);

    /// 每個批次結束時會問一次。回傳 `true` 就會在 commit 完當前批次後停止。
    fn cancelled(&self) -> bool {
        false
    }
}

/// 什麼都不做的實作。
pub struct NoProgress;

impl ProgressSink for NoProgress {
    fn report(&self, _: &ImportProgress) {}
}

#[derive(Debug, Clone)]
pub struct ImportOptions {
    /// 每幾筆 commit 一次。太小會慢，太大會讓取消變遲鈍、記憶體佔用上升。
    pub batch_size: usize,
    /// 每處理幾筆回報一次進度。
    pub report_every: u64,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            batch_size: 1_000,
            report_every: 2_000,
        }
    }
}

// ---------------------------------------------------------------- 型別轉換

/// `DictEntry` → 資料庫寫入結構。借用而不複製字串，百萬筆時差很多。
fn to_write<'a>(entry: &'a DictEntry) -> EntryWrite<'a> {
    EntryWrite {
        lang: &entry.lang,
        headword: &entry.headword,
        pos: &entry.pos,
        freq_rank: entry.freq_rank,
        cefr: entry.cefr.as_deref(),
        senses: entry
            .senses
            .iter()
            .map(|s| NewSense {
                gloss: &s.gloss,
                gloss_lang: &s.gloss_lang,
                translation: s.translation.as_deref(),
                register: s.register.as_deref(),
                domain: s.domain.as_deref(),
                examples: s
                    .examples
                    .iter()
                    .map(|e| NewExample {
                        text: &e.text,
                        translation: e.translation.as_deref(),
                    })
                    .collect(),
            })
            .collect(),
        pronunciations: entry
            .pronunciations
            .iter()
            .map(|p| NewPronunciation {
                accent: p.accent.as_deref(),
                ipa: p.ipa.as_deref(),
                // 只記網址，實際下載是獨立的步驟——完整音檔集有好幾 GB，
                // 但使用者真正需要的只有牌組裡那幾百個字
                audio_url: p.audio_url.as_deref(),
                audio_path: None,
                audio_license: p.audio_license.as_deref(),
                is_synthetic: false,
            })
            .collect(),
        forms: entry
            .forms
            .iter()
            .map(|(f, tag)| (f.as_str(), tag.as_str()))
            .collect(),
        tags: entry.tags.iter().map(String::as_str).collect(),
    }
}

/// 登記來源。
pub async fn register_source(db: &Db, meta: &SourceMeta) -> Result<SourceId> {
    Ok(dict::upsert_source(
        db,
        NewSource {
            slug: &meta.slug,
            name: &meta.name,
            license: meta.license.as_deref(),
            attribution: meta.attribution.as_deref(),
            homepage: meta.homepage.as_deref(),
            version: meta.version.as_deref(),
        },
        OffsetDateTime::now_utc(),
    )
    .await?)
}

// ---------------------------------------------------------------- 匯入

/// 批次寫入詞條。
///
/// 解析失敗的項目會被計入 `failed` 並跳過，不會中斷整批匯入。
/// 資料庫層級的錯誤（磁碟滿、schema 不符）則會直接中止——
/// 那代表環境有問題，繼續跑只是浪費時間。
///
/// 重複匯入同一個來源是冪等的：每個詞條在這一輪第一次被碰到時清空重寫，
/// 之後同一個詞條的其他詞源接在後面。**不能**改成每筆各自 `Replace`：
/// 一份 dump 裡同一個 `(lang, text, pos)` 會出現好幾次（Wiktionary 的
/// 多詞源條目），那樣後面的會把前面的洗掉。詳見 [`dict::WriteMode`]。
pub async fn import_entries<I>(
    db: &Db,
    source: SourceId,
    entries: I,
    opts: &ImportOptions,
    sink: &dyn ProgressSink,
) -> Result<ImportProgress>
where
    I: IntoIterator<Item = std::result::Result<DictEntry, DictError>>,
{
    let mut progress = ImportProgress::default();
    let mut iter = entries.into_iter();
    let mut exhausted = false;
    // 這一輪已經清空過的詞條。要跨批次活著——同一個詞的兩個詞源
    // 落在不同批次是常態。百萬筆量級大約幾十 MB，比起 dump 本身可以忽略。
    let mut seen = std::collections::HashSet::new();

    while !exhausted {
        let mut tx = db.pool().begin().await?;
        let mut in_batch = 0usize;

        while in_batch < opts.batch_size {
            let Some(item) = iter.next() else {
                exhausted = true;
                break;
            };
            progress.processed += 1;

            match item {
                Err(e) => {
                    tracing::debug!(error = %e, "跳過一筆無法解析的資料");
                    progress.failed += 1;
                }
                Ok(entry) if !entry.is_usable() => progress.skipped += 1,
                Ok(entry) => {
                    dict::write_entry(
                        &mut tx,
                        source,
                        &to_write(&entry),
                        dict::WriteMode::Batch(&mut seen),
                    )
                    .await?;
                    progress.imported += 1;
                    in_batch += 1;
                }
            }

            if progress.processed % opts.report_every == 0 {
                sink.report(&progress);
            }
        }

        tx.commit().await?;

        // 取消只在批次邊界檢查：這樣已寫入的資料一定是完整的批次，
        // 不會留下寫到一半的詞條。
        if !exhausted && sink.cancelled() {
            progress.cancelled = true;
            break;
        }
    }

    sink.report(&progress);
    Ok(progress)
}

// ---------------------------------------------------------------- 檔案入口

/// 邊讀邊記位元組數，讓進度條有東西可以顯示。
struct CountingReader<R> {
    inner: R,
    count: Arc<AtomicU64>,
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.count.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

/// 幫進度補上位元組資訊的轉接層。
struct ByteAwareSink<'a> {
    inner: &'a dyn ProgressSink,
    count: Arc<AtomicU64>,
    total: u64,
}

impl ProgressSink for ByteAwareSink<'_> {
    fn report(&self, progress: &ImportProgress) {
        let mut p = *progress;
        p.bytes_read = self.count.load(Ordering::Relaxed);
        p.bytes_total = self.total;
        self.inner.report(&p);
    }

    fn cancelled(&self) -> bool {
        self.inner.cancelled()
    }
}

/// 匯入 kaikki.org 的 Wiktionary JSONL。
pub async fn import_wiktionary_jsonl(
    db: &Db,
    path: &Path,
    lang: &str,
    opts: &ImportOptions,
    sink: &dyn ProgressSink,
) -> Result<ImportProgress> {
    let meta = SourceMeta::wiktionary(lang);
    let source = register_source(db, &meta).await?;

    let file = File::open(path)?;
    let total = file.metadata().map(|m| m.len()).unwrap_or(0);
    let count = Arc::new(AtomicU64::new(0));
    let reader = BufReader::with_capacity(
        1 << 20,
        CountingReader {
            inner: file,
            count: Arc::clone(&count),
        },
    );

    let byte_sink = ByteAwareSink {
        inner: sink,
        count: Arc::clone(&count),
        total,
    };

    let mut progress = import_entries(
        db,
        source,
        wordforge_dict::kaikki::parse_reader(reader),
        opts,
        &byte_sink,
    )
    .await?;

    progress.bytes_read = count.load(Ordering::Relaxed);
    progress.bytes_total = total;
    Ok(progress)
}

/// 匯入 CSV / TSV 單字表。
///
/// 這種檔案通常只有幾百行，一次全讀進記憶體沒問題。
pub async fn import_csv(
    db: &Db,
    path: &Path,
    lang: &str,
    delimiter: u8,
    source_name: &str,
    opts: &ImportOptions,
    sink: &dyn ProgressSink,
) -> Result<ImportProgress> {
    let slug = format!(
        "csv-{}",
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("untitled")
    );
    let meta = SourceMeta {
        slug,
        name: source_name.to_string(),
        // 使用者自己的單字表，授權由使用者自行認定
        license: None,
        attribution: None,
        homepage: None,
        version: None,
    };
    let source = register_source(db, &meta).await?;

    let entries = wordforge_dict::tabular::parse(File::open(path)?, lang, delimiter)?;
    import_entries(db, source, entries.into_iter().map(Ok), opts, sink).await
}

/// 詞頻表的格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreqFormat {
    /// 一行一個字，行號就是排名
    RankedList,
    /// `word<TAB>count`
    TabCounts,
    /// `word,count`
    CommaCounts,
    /// `word count`（OpenSubtitles 的 FrequencyWords 用這種）
    SpaceCounts,
}

/// 套用詞頻表。回傳實際更新到的詞條數。
pub async fn import_freq_list(db: &Db, path: &Path, lang: &str, format: FreqFormat) -> Result<u64> {
    let reader = BufReader::new(File::open(path)?);
    let table = match format {
        FreqFormat::RankedList => wordforge_dict::freq::load_ranked_list(reader)?,
        FreqFormat::TabCounts => wordforge_dict::freq::load_counts(reader, '\t')?,
        FreqFormat::CommaCounts => wordforge_dict::freq::load_counts(reader, ',')?,
        FreqFormat::SpaceCounts => wordforge_dict::freq::load_counts(reader, ' ')?,
    };
    Ok(dict::apply_freq_ranks(db, lang, &table).await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicBool;
    use wordforge_dict::{ExampleEntry, SenseEntry};

    /// 記錄每次回報的測試用 sink，可設定在第幾次回報後取消。
    #[derive(Default)]
    struct Recorder {
        reports: Mutex<Vec<ImportProgress>>,
        cancel_after: Option<u64>,
        cancelled: AtomicBool,
    }

    impl ProgressSink for Recorder {
        fn report(&self, p: &ImportProgress) {
            self.reports.lock().unwrap().push(*p);
            if let Some(n) = self.cancel_after
                && p.imported >= n
            {
                self.cancelled.store(true, Ordering::SeqCst);
            }
        }

        fn cancelled(&self) -> bool {
            self.cancelled.load(Ordering::SeqCst)
        }
    }

    fn entry(word: &str) -> DictEntry {
        DictEntry {
            lang: "en".into(),
            headword: word.into(),
            pos: "noun".into(),
            senses: vec![SenseEntry {
                gloss: format!("meaning of {word}"),
                gloss_lang: "en".into(),
                examples: vec![ExampleEntry {
                    text: format!("A sentence with {word}."),
                    translation: None,
                }],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    async fn setup() -> (Db, SourceId) {
        let db = Db::open_in_memory().await.unwrap();
        let source = register_source(&db, &SourceMeta::wiktionary("en"))
            .await
            .unwrap();
        (db, source)
    }

    #[tokio::test]
    async fn imports_entries_and_counts_them() {
        let (db, source) = setup().await;
        let entries = vec![Ok(entry("apple")), Ok(entry("banana"))];

        let p = import_entries(&db, source, entries, &ImportOptions::default(), &NoProgress)
            .await
            .unwrap();

        assert_eq!(p.imported, 2);
        assert_eq!(p.processed, 2);
        assert!(!p.cancelled);

        let stats = dict::stats(&db).await.unwrap();
        assert_eq!(stats.lemmas, 2);
        assert_eq!(stats.senses, 2);
    }

    /// 壞掉的行與沒有釋義的詞條都不該中斷整批匯入。
    #[tokio::test]
    async fn bad_rows_are_counted_not_fatal() {
        let (db, source) = setup().await;
        let entries = vec![
            Ok(entry("apple")),
            Err(DictError::Malformed {
                line: 2,
                reason: "壞掉".into(),
            }),
            Ok(DictEntry {
                headword: "redirect".into(),
                ..Default::default()
            }),
            Ok(entry("banana")),
        ];

        let p = import_entries(&db, source, entries, &ImportOptions::default(), &NoProgress)
            .await
            .unwrap();

        assert_eq!(p.processed, 4);
        assert_eq!(p.imported, 2);
        assert_eq!(p.failed, 1);
        assert_eq!(p.skipped, 1, "沒有釋義的詞條要跳過而不是寫進去");
    }

    /// 取消之後，已經 commit 的批次必須保留。
    #[tokio::test]
    async fn cancelling_keeps_committed_batches() {
        let (db, source) = setup().await;
        let entries: Vec<_> = (0..25).map(|i| Ok(entry(&format!("word{i:03}")))).collect();

        let sink = Recorder {
            cancel_after: Some(5),
            ..Default::default()
        };
        let opts = ImportOptions {
            batch_size: 5,
            report_every: 5,
        };

        let p = import_entries(&db, source, entries, &opts, &sink)
            .await
            .unwrap();

        assert!(p.cancelled, "應該要中途停下來");
        assert!(p.imported < 25, "取消之後不該把剩下的都匯入");
        assert!(p.imported >= 5, "已經 commit 的批次要保留");

        let stats = dict::stats(&db).await.unwrap();
        assert_eq!(
            stats.lemmas, p.imported as i64,
            "資料庫裡的筆數要跟回報的一致"
        );
    }

    #[tokio::test]
    async fn progress_is_reported_periodically() {
        let (db, source) = setup().await;
        let entries: Vec<_> = (0..10).map(|i| Ok(entry(&format!("w{i}")))).collect();
        let sink = Recorder::default();

        import_entries(
            &db,
            source,
            entries,
            &ImportOptions {
                batch_size: 100,
                report_every: 4,
            },
            &sink,
        )
        .await
        .unwrap();

        let reports = sink.reports.lock().unwrap();
        // 第 4、8 筆各一次，結束時一次
        assert_eq!(reports.len(), 3);
        assert_eq!(reports.last().unwrap().imported, 10);
    }

    #[tokio::test]
    async fn fraction_needs_a_known_total() {
        let p = ImportProgress {
            bytes_read: 50,
            bytes_total: 200,
            ..Default::default()
        };
        assert_eq!(p.fraction(), Some(0.25));
        assert_eq!(ImportProgress::default().fraction(), None);
    }

    /// 建立一個測試用的暫存檔。回傳 (路徑, 用完要刪的目錄)。
    fn temp_file(name: &str, content: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("wordforge-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        (path, dir)
    }

    /// 走完整條實際路徑：開檔 → 串流解析 → 寫入 → 查得到。
    #[tokio::test]
    async fn imports_a_jsonl_file_end_to_end() {
        let jsonl = concat!(
            r#"{"word":"cat","pos":"noun","lang_code":"en","senses":[{"glosses":["A small feline"]}],"#,
            r#""sounds":[{"ipa":"/kæt/","tags":["UK"]}],"forms":[{"form":"cats","tags":["plural"]}]}"#,
            "\n",
            r#"{"word":"dog","pos":"noun","lang_code":"en","senses":[{"glosses":["A domestic canine"]}]}"#,
            "\n",
            "{broken json}\n",
            r#"{"word":"ghost","senses":[]}"#,
            "\n",
        );
        let (path, dir) = temp_file("sample.jsonl", jsonl);
        let db = Db::open_in_memory().await.unwrap();

        let p = import_wiktionary_jsonl(&db, &path, "en", &ImportOptions::default(), &NoProgress)
            .await
            .unwrap();

        assert_eq!(p.imported, 2);
        assert_eq!(p.failed, 1, "壞掉的行要計數而不是中斷匯入");
        assert_eq!(p.skipped, 1, "沒有釋義的詞條要跳過");
        assert!(p.bytes_total > 0, "檔案大小應該讀得到，進度條才有意義");
        assert_eq!(p.bytes_read, p.bytes_total, "讀完整個檔案");

        // 來源與授權要正確登記，UI 才有東西可以標示
        let stats = dict::stats(&db).await.unwrap();
        assert_eq!(stats.sources[0].slug, "wiktionary-en");
        assert_eq!(stats.sources[0].license.as_deref(), Some("CC BY-SA 4.0"));

        // 查得到，而且詞形變化也對得回原形
        let hits = wordforge_db::dict::search(&db, "en", "cats", 1, 10)
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].text, "cat");
        assert_eq!(hits[0].gloss.as_deref(), Some("A small feline"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn imports_a_csv_file_end_to_end() {
        let (path, dir) = temp_file(
            "words.csv",
            "word,translation,ipa\napple,蘋果,/ˈæp.əl/\nbook,書,\n",
        );
        let db = Db::open_in_memory().await.unwrap();

        let p = import_csv(
            &db,
            &path,
            "en",
            b',',
            "我的單字表",
            &ImportOptions::default(),
            &NoProgress,
        )
        .await
        .unwrap();

        assert_eq!(p.imported, 2);

        let hits = wordforge_db::dict::search(&db, "en", "apple", 1, 10)
            .await
            .unwrap();
        assert_eq!(hits[0].translation.as_deref(), Some("蘋果"));

        // 自製單字表沒有授權資訊，不該亂填一個
        let stats = dict::stats(&db).await.unwrap();
        assert_eq!(stats.sources[0].name, "我的單字表");
        assert_eq!(stats.sources[0].license, None);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 詞頻表套用到已匯入的詞條上。
    #[tokio::test]
    async fn freq_list_updates_imported_words() {
        let (csv, csv_dir) = temp_file("w.csv", "word,translation\napple,蘋果\nzebra,斑馬\n");
        let (freq, freq_dir) = temp_file("freq.txt", "the\napple\nof\n");
        let db = Db::open_in_memory().await.unwrap();

        import_csv(
            &db,
            &csv,
            "en",
            b',',
            "表",
            &ImportOptions::default(),
            &NoProgress,
        )
        .await
        .unwrap();

        let updated = import_freq_list(&db, &freq, "en", FreqFormat::RankedList)
            .await
            .unwrap();

        assert_eq!(updated, 1, "只有 apple 兩邊都有");
        let hits = wordforge_db::dict::search(&db, "en", "apple", 1, 5)
            .await
            .unwrap();
        assert_eq!(hits[0].freq_rank, Some(2), "apple 在詞頻表的第 2 行");

        std::fs::remove_dir_all(&csv_dir).ok();
        std::fs::remove_dir_all(&freq_dir).ok();
    }

    #[tokio::test]
    async fn missing_file_is_a_clear_error() {
        let db = Db::open_in_memory().await.unwrap();
        let err = import_wiktionary_jsonl(
            &db,
            std::path::Path::new("/nonexistent/nope.jsonl"),
            "en",
            &ImportOptions::default(),
            &NoProgress,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ImportError::Io(_)));
    }

    #[tokio::test]
    async fn reimporting_the_same_file_does_not_duplicate() {
        let (db, source) = setup().await;
        let make = || vec![Ok(entry("apple")), Ok(entry("banana"))];

        import_entries(&db, source, make(), &ImportOptions::default(), &NoProgress)
            .await
            .unwrap();
        import_entries(&db, source, make(), &ImportOptions::default(), &NoProgress)
            .await
            .unwrap();

        let stats = dict::stats(&db).await.unwrap();
        assert_eq!(stats.lemmas, 2);
        assert_eq!(stats.senses, 2, "重複匯入不該讓釋義變兩倍");
    }

    /// 這條測試存在的理由是它曾經是錯的：Wiktionary 的 `cat` 有好幾個
    /// 詞源各自一筆 `pos="noun"`，而 lemma 的鍵只到 `(lang, text, pos)`。
    /// 每筆進來都先刪光同來源的舊釋義，所以最後那個詞源（catapult、
    /// category 那些縮寫）贏了，「貓」整組被洗掉。
    ///
    /// `batch_size` 刻意設成 1，讓兩個詞源落在不同的 transaction——
    /// 「這一輪清過誰」必須跨批次記得，不然這個 bug 會原封不動回來。
    #[tokio::test]
    async fn every_etymology_of_the_same_word_survives_one_import() {
        let (db, source) = setup().await;

        let etymology = |gloss: &str| DictEntry {
            lang: "en".into(),
            headword: "cat".into(),
            pos: "noun".into(),
            senses: vec![SenseEntry {
                gloss: gloss.into(),
                gloss_lang: "en".into(),
                ..Default::default()
            }],
            ..Default::default()
        };

        let entries = vec![
            Ok(etymology("A domesticated feline animal.")),
            Ok(etymology("Abbreviation of catapult.")),
            Ok(etymology("Abbreviation of category.")),
        ];
        let opts = ImportOptions {
            batch_size: 1,
            ..ImportOptions::default()
        };

        import_entries(&db, source, entries, &opts, &NoProgress)
            .await
            .unwrap();

        let stats = dict::stats(&db).await.unwrap();
        assert_eq!(stats.lemmas, 1, "三個詞源共用同一個 (lang, text, pos)");
        assert_eq!(stats.senses, 3, "三個詞源的釋義都要留著");

        // 排序要照匯入順序延續，不能三個都從 0 開始
        let entries = dict::glossary(&db, "en", &["cat".to_string()])
            .await
            .unwrap();
        assert_eq!(
            entries[0].gloss.as_deref(),
            Some("A domesticated feline animal."),
            "第一個詞源的第一條釋義要排在最前面"
        );
    }
}

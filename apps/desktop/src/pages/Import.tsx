import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  cancelImport,
  type DictStats,
  dictionaryStats,
  errorMessage,
  IMPORT_EVENTS,
  IMPORT_KINDS,
  type ImportKind,
  type ImportProgress,
  importRunning,
  startImport,
} from "../api";

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 ** 2) return `${(n / 1024).toFixed(0)} KB`;
  if (n < 1024 ** 3) return `${(n / 1024 ** 2).toFixed(1)} MB`;
  return `${(n / 1024 ** 3).toFixed(2)} GB`;
}

/**
 * 匯入頁。
 *
 * 進度來自後端事件而不是輪詢：一份 Wiktionary 有上百萬筆，
 * 匯入要跑好幾分鐘，使用者需要看得到東西在動。
 */
export default function Import() {
  const [kind, setKind] = useState<ImportKind>("wiktionary_jsonl");
  const [path, setPath] = useState<string | null>(null);
  const [lang, setLang] = useState("en");
  const [running, setRunning] = useState(false);
  const [progress, setProgress] = useState<ImportProgress | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [stats, setStats] = useState<DictStats | null>(null);

  const loadStats = useCallback(async () => {
    try {
      setStats(await dictionaryStats());
    } catch (e) {
      setError(errorMessage(e));
    }
  }, []);

  useEffect(() => {
    void loadStats();
    void importRunning().then(setRunning);

    // listen 回傳的 unlisten 是 Promise，卸載時要記得等它
    const subscriptions = [
      listen<ImportProgress>(IMPORT_EVENTS.progress, (e) => setProgress(e.payload)),
      listen<ImportProgress>(IMPORT_EVENTS.done, (e) => {
        setProgress(e.payload);
        setRunning(false);
        setMessage(
          e.payload.cancelled
            ? `已中止，保留了 ${e.payload.imported} 筆`
            : `完成：匯入 ${e.payload.imported} 筆，跳過 ${e.payload.skipped} 筆，失敗 ${e.payload.failed} 筆`,
        );
        void loadStats();
      }),
      listen<string>(IMPORT_EVENTS.error, (e) => {
        setRunning(false);
        setError(e.payload);
      }),
    ];

    return () => {
      void Promise.all(subscriptions).then((fns) => fns.forEach((f) => f()));
    };
  }, [loadStats]);

  async function pickFile() {
    const spec = IMPORT_KINDS.find((k) => k.value === kind);
    const picked = await openDialog({
      multiple: false,
      directory: false,
      filters: spec ? [{ name: spec.label, extensions: spec.extensions }] : undefined,
    });
    if (typeof picked === "string") {
      setPath(picked);
      setMessage(null);
      setError(null);
    }
  }

  async function begin() {
    if (!path) return;
    try {
      setError(null);
      setMessage(null);
      setProgress(null);
      setRunning(true);
      await startImport(path, kind, lang);
    } catch (e) {
      setRunning(false);
      setError(errorMessage(e));
    }
  }

  const fraction =
    progress && progress.bytes_total > 0 ? progress.bytes_read / progress.bytes_total : null;

  return (
    <div className="import">
      <section className="panel">
        <h2>匯入字典</h2>

        <label>
          格式
          <select
            value={kind}
            onChange={(e) => setKind(e.target.value as ImportKind)}
            disabled={running}
          >
            {IMPORT_KINDS.map((k) => (
              <option key={k.value} value={k.value}>
                {k.label}
              </option>
            ))}
          </select>
        </label>

        <label>
          語言代碼
          <input
            value={lang}
            onChange={(e) => setLang(e.target.value)}
            disabled={running}
            placeholder="en"
            size={6}
          />
        </label>

        <div className="row">
          <button onClick={pickFile} disabled={running}>
            選擇檔案…
          </button>
          <span className="path muted">{path ?? "尚未選擇"}</span>
        </div>

        <div className="row">
          <button className="primary" onClick={begin} disabled={!path || running}>
            開始匯入
          </button>
          <button onClick={() => cancelImport()} disabled={!running}>
            取消
          </button>
        </div>

        {progress && (
          <div className="progress">
            {fraction != null && (
              <progress value={fraction} max={1}>
                {Math.round(fraction * 100)}%
              </progress>
            )}
            <p className="muted">
              已處理 {progress.processed.toLocaleString()} 筆 · 匯入{" "}
              {progress.imported.toLocaleString()} · 跳過 {progress.skipped.toLocaleString()} ·
              失敗 {progress.failed.toLocaleString()}
              {progress.bytes_total > 0 &&
                ` · ${formatBytes(progress.bytes_read)} / ${formatBytes(progress.bytes_total)}`}
            </p>
          </div>
        )}

        {message && <p className="ok">{message}</p>}
        {error && <p className="error">{error}</p>}
      </section>

      <section className="panel">
        <h2>目前的字典</h2>
        {stats ? (
          <>
            <dl className="stats">
              <div>
                <dt>詞條</dt>
                <dd>{stats.lemmas.toLocaleString()}</dd>
              </div>
              <div>
                <dt>釋義</dt>
                <dd>{stats.senses.toLocaleString()}</dd>
              </div>
              <div>
                <dt>有音檔</dt>
                <dd>{stats.with_audio.toLocaleString()}</dd>
              </div>
            </dl>

            {stats.sources.length > 0 ? (
              <ul className="sources">
                {stats.sources.map((s) => (
                  <li key={s.slug}>
                    <strong>{s.name}</strong>
                    <span className="muted">
                      {" "}
                      {s.lemma_count.toLocaleString()} 筆
                      {s.license ? ` · ${s.license}` : " · 授權未標示"}
                    </span>
                  </li>
                ))}
              </ul>
            ) : (
              <p className="muted">還沒有匯入任何字典。</p>
            )}
          </>
        ) : (
          <p className="muted">載入中…</p>
        )}
      </section>

      <section className="panel note">
        <h2>從哪裡取得字典</h2>
        <p>
          本專案不散布任何字典資料。商業字典（Cambridge、Oxford 等）的釋義與錄音受著作權保護，
          請改用授權明確的來源：
        </p>
        <ul>
          <li>
            <strong>Wiktionary</strong>（CC BY-SA 4.0）— 從{" "}
            <code>kaikki.org/dictionary/English/</code> 下載 JSONL
          </li>
          <li>
            <strong>詞頻表</strong> — wordfreq、SUBTLEX；90% 法則靠它決定先學哪些字
          </li>
          <li>
            <strong>你自己的單字表</strong> — CSV，只有 <code>word</code> 欄位是必填
          </li>
        </ul>
        <p className="muted">
          匯入 CC BY-SA 的內容時，App 會在釋義下方標示出處——這是授權的要求，請勿移除。
        </p>
      </section>
    </div>
  );
}

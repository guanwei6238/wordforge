import { useCallback, useEffect, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  deleteMaterial,
  errorMessage,
  importMaterial,
  languageName,
  listMaterials,
  type Material,
  MATERIAL_KIND_LABELS,
  materialCoverage,
} from "../api";

/**
 * 教材管理。
 *
 * 這是跟閱讀測驗相反的功能：閱讀測驗照你的程度當場生一篇，這裡是把
 * 模型綁死在你指定的課本上。考試只考課本，模型講到課本以外的東西就是干擾。
 *
 * App 不內建也不散布任何教材——跟字典是同一條政策。使用者匯入自己
 * 合法取得的檔案，`license_note` 讓他自己記下這份東西能不能分享。
 */
export default function MaterialManager() {
  const [materials, setMaterials] = useState<Material[]>([]);
  const [coverage, setCoverage] = useState<Record<number, [number, number]>>({});
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const all = await listMaterials();
      setMaterials(all);
      const pairs = await Promise.all(
        all.map(async (m) => [m.id, await materialCoverage(m.id)] as const),
      );
      setCoverage(Object.fromEntries(pairs));
    } catch (e) {
      setError(errorMessage(e));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  async function pickAndImport() {
    const picked = await openDialog({
      multiple: false,
      filters: [
        {
          name: "教材",
          extensions: ["txt", "md", "epub", "pdf", "srt", "vtt", "html", "htm", "xhtml"],
        },
      ],
    });
    if (typeof picked !== "string") {
      return;
    }

    setBusy(true);
    setError(null);
    setMessage(null);
    try {
      const result = await importMaterial(picked);
      setMessage(
        `匯入完成：${result.chunks} 段、${result.vocab.toLocaleString()} 個字` +
          (result.unmatched_tokens > 0
            ? `（有 ${result.unmatched_tokens.toLocaleString()} 個詞元字典裡查不到）`
            : ""),
      );
      await load();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  async function remove(m: Material) {
    setBusy(true);
    setError(null);
    try {
      await deleteMaterial(m.id);
      setMessage(`已刪除「${m.title}」`);
      await load();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="panel">
      <h2>教材</h2>
      <p className="muted hint">
        匯入自己的課本，出題時就能限定「只從這本書取材」。支援純文字、EPUB、PDF、字幕檔。
        PDF 只讀得到文字層——掃描的書要先做 OCR。
      </p>

      <button onClick={pickAndImport} disabled={busy}>
        {busy ? "處理中…" : "匯入教材檔案"}
      </button>

      {materials.length > 0 && (
        <ul className="materials">
          {materials.map((m) => {
            const [total, known] = coverage[m.id] ?? [0, 0];
            const ratio = total > 0 ? Math.round((known / total) * 100) : null;
            return (
              <li key={m.id}>
                <div className="material-head">
                  <strong>{m.title}</strong>
                  <span className="tag">{MATERIAL_KIND_LABELS[m.kind] ?? m.kind}</span>
                  <span className="tag">{languageName(m.lang)}</span>
                </div>
                <p className="muted">
                  {m.chunk_count} 段 · {m.vocab_count.toLocaleString()} 個字
                  {ratio !== null && ` · 你已掌握 ${ratio}%`}
                </p>
                {m.license_note && <p className="muted hint">備註：{m.license_note}</p>}
                <button onClick={() => remove(m)} disabled={busy}>
                  刪除
                </button>
              </li>
            );
          })}
        </ul>
      )}

      {materials.length === 0 && (
        <p className="muted">還沒有教材。匯入之後，練習頁就會多出「只從這本書出題」的選項。</p>
      )}

      {message && <p className="ok">{message}</p>}
      {error && <p className="error">{error}</p>}
    </section>
  );
}

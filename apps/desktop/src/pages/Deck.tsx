import { useCallback, useEffect, useState } from "react";
import { addWordsByTag, deckTags, errorMessage, tagLabel, type TagSummary } from "../api";
import AudioDownload from "../components/AudioDownload";
import PlacementTest from "../components/PlacementTest";

/** 一次加入的張數選項。500 字大約是兩三週的量。 */
const BATCH_SIZES = [100, 300, 500, 1000];

/**
 * 牌組頁：依考試範圍批次加入單字。
 *
 * 一個字一個字加不實際——國中範圍就有一千六百個字。
 * 加入時依詞頻由常用到罕見，所以先學到的一定是最划算的那些。
 */
export default function Deck() {
  const [tags, setTags] = useState<TagSummary[]>([]);
  const [limit, setLimit] = useState(300);
  const [busy, setBusy] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setTags(await deckTags());
      setError(null);
    } catch (e) {
      setError(errorMessage(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  async function add(tag: string) {
    setBusy(tag);
    setMessage(null);
    try {
      const added = await addWordsByTag(tag, limit);
      setMessage(
        added > 0
          ? `加入 ${added} 個「${tagLabel(tag)}」的單字`
          : `「${tagLabel(tag)}」範圍內的字都已經在牌組裡了`,
      );
      await refresh();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  }

  return (
    <div className="deck">
      <PlacementTest onFinished={refresh} />

      <AudioDownload />

      <section className="panel">
        <h2>依範圍加入單字</h2>
        <p className="muted">
          加入時會依詞頻由常用排到罕見，所以先學到的一定是最常用的字。
          已經在牌組裡的字不會被重置。
        </p>

        <label>
          每次加入
          <select value={limit} onChange={(e) => setLimit(Number(e.target.value))}>
            {BATCH_SIZES.map((n) => (
              <option key={n} value={n}>
                {n} 個字
              </option>
            ))}
          </select>
        </label>

        {tags.length === 0 ? (
          <p className="muted">
            字典裡沒有考試範圍標籤。到「匯入」頁載入 ECDICT 就會有國中、學測、
            多益等範圍可以選。
          </p>
        ) : (
          <ul className="tag-list">
            {tags.map((t) => {
              const done = t.in_deck >= t.total;
              return (
                <li key={t.tag}>
                  <span className="tag-name">{tagLabel(t.tag)}</span>
                  <span className="muted tag-count">
                    {t.in_deck.toLocaleString()} / {t.total.toLocaleString()}
                  </span>
                  <progress value={t.in_deck} max={t.total} />
                  <button onClick={() => add(t.tag)} disabled={busy !== null || done}>
                    {done ? "已全部加入" : busy === t.tag ? "加入中…" : "加入"}
                  </button>
                </li>
              );
            })}
          </ul>
        )}

        {message && <p className="ok">{message}</p>}
        {error && <p className="error">{error}</p>}
      </section>
    </div>
  );
}

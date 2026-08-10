import { useEffect, useState } from "react";
import {
  addLemmaToDeck,
  errorMessage,
  type SearchHit,
  searchWords,
  tagLabel,
  type WordDetail,
  wordDetail,
} from "../api";
import SpeakButton from "../components/SpeakButton";

/**
 * 查字典頁。
 *
 * 搜尋輸入做了 200ms 的 debounce：字典動輒上百萬筆，
 * 每敲一個字母就查一次會讓打字明顯卡頓。
 */
export default function Dictionary() {
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [selected, setSelected] = useState<WordDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [searching, setSearching] = useState(false);

  useEffect(() => {
    const q = query.trim();
    if (!q) {
      setHits([]);
      return;
    }
    let cancelled = false;
    setSearching(true);
    const timer = setTimeout(async () => {
      try {
        const result = await searchWords(q);
        // 慢回來的舊查詢不能覆蓋新查詢的結果
        if (!cancelled) {
          setHits(result);
          setError(null);
        }
      } catch (e) {
        if (!cancelled) setError(errorMessage(e));
      } finally {
        if (!cancelled) setSearching(false);
      }
    }, 200);

    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [query]);

  async function open(lemmaId: number) {
    try {
      setSelected(await wordDetail(lemmaId));
      setError(null);
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  async function addToDeck(lemmaId: number) {
    try {
      await addLemmaToDeck(lemmaId);
      setHits((hs) => hs.map((h) => (h.lemma_id === lemmaId ? { ...h, in_deck: true } : h)));
      setSelected((d) => (d && d.lemma_id === lemmaId ? { ...d, in_deck: true } : d));
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  return (
    <div className="dictionary">
      <input
        className="search"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        placeholder="查單字…（支援 ran → run 這類詞形變化）"
        aria-label="查單字"
        autoFocus
      />

      {error && <p className="error">{error}</p>}

      <div className="dict-body">
        <ul className="hits">
          {hits.map((h) => (
            <li key={h.lemma_id}>
              <button
                className={selected?.lemma_id === h.lemma_id ? "hit selected" : "hit"}
                onClick={() => open(h.lemma_id)}
              >
                <span className="hit-word">{h.text}</span>
                {h.pos && <span className="tag">{h.pos}</span>}
                {h.cefr && <span className="tag">{h.cefr}</span>}
                {h.tags.slice(0, 2).map((t) => (
                  <span key={t} className="tag exam">
                    {tagLabel(t)}
                  </span>
                ))}
                {h.in_deck && <span className="tag in-deck">已在牌組</span>}
                <span className="hit-gloss">{h.translation ?? h.gloss ?? ""}</span>
              </button>
            </li>
          ))}
          {!searching && query.trim() && hits.length === 0 && (
            <li className="muted empty">
              查不到這個字。字典是空的嗎？到「匯入」頁載入一份字典。
            </li>
          )}
        </ul>

        {selected && (
          <article className="detail">
            <header>
              <h2>{selected.text}</h2>
              <SpeakButton
                text={selected.text}
                audioPath={selected.pronunciations[0]?.audio_path}
              />
              {selected.pos && <span className="tag">{selected.pos}</span>}
              {selected.freq_rank != null && (
                <span className="tag" title="詞頻排名，越小越常用">
                  #{selected.freq_rank}
                </span>
              )}
              {selected.tags.map((t) => (
                <span key={t} className="tag exam" title={t}>
                  {tagLabel(t)}
                </span>
              ))}
              <button
                className="add"
                disabled={selected.in_deck}
                onClick={() => addToDeck(selected.lemma_id)}
              >
                {selected.in_deck ? "已在牌組" : "加入牌組"}
              </button>
            </header>

            {selected.pronunciations.length > 0 && (
              <p className="prons">
                {selected.pronunciations.map((p, i) => (
                  <span key={i} className="pron">
                    {p.accent && <span className="tag">{p.accent}</span>}
                    {p.ipa}
                    {p.is_synthetic && <span className="tag">合成音</span>}
                  </span>
                ))}
              </p>
            )}

            <ol className="senses">
              {selected.senses.map((s, i) => (
                <li key={i}>
                  <p className="gloss">
                    {s.pos && <span className="tag pos">{s.pos}</span>}
                    {s.gloss}
                  </p>
                  {s.translation && <p className="translation">{s.translation}</p>}
                  {(s.register || s.domain) && (
                    <p className="labels">
                      {s.register && <span className="tag">{s.register}</span>}
                      {s.domain && <span className="tag">{s.domain}</span>}
                    </p>
                  )}
                  {s.examples.map((ex, j) => (
                    <p key={j} className="example">
                      {ex.text}
                      {ex.translation && <span className="muted"> — {ex.translation}</span>}
                    </p>
                  ))}
                  {/* CC BY-SA 的標示義務，不能省略 */}
                  {s.attribution && <p className="attribution">— {s.attribution}</p>}
                </li>
              ))}
            </ol>

            {selected.forms.length > 0 && (
              <p className="forms">
                <span className="muted">變化形：</span>
                {selected.forms.map(([form, tag], i) => (
                  <span key={i} className="tag" title={tag}>
                    {form}
                  </span>
                ))}
              </p>
            )}
          </article>
        )}
      </div>
    </div>
  );
}

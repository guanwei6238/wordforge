import { type FormEvent, useCallback, useEffect, useState } from "react";
import {
  addWord,
  type CardView,
  errorMessage,
  listDueCards,
  RATING,
  type Rating,
  reviewCard,
  type StudyStats,
  studyStats,
} from "../api";
import SpeakButton from "../components/SpeakButton";

/**
 * 複習頁：只做「看字 → 想意思 → 自評」這一條路徑。
 *
 * 閱讀理解、AI 對話、寫作批改會是各自獨立的頁面，
 * 但都建立在同一份卡片資料之上。
 */
export default function Review() {
  const [queue, setQueue] = useState<CardView[]>([]);
  const [stats, setStats] = useState<StudyStats | null>(null);
  const [revealed, setRevealed] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [shownAt, setShownAt] = useState(() => Date.now());
  const [newWord, setNewWord] = useState("");

  const refresh = useCallback(async () => {
    try {
      const [cards, s] = await Promise.all([listDueCards(), studyStats()]);
      setQueue(cards);
      setStats(s);
      setError(null);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const current = queue[0];

  const grade = useCallback(
    async (rating: Rating) => {
      if (!current) return;
      try {
        // 作答時間是判斷「真的會」還是「猜到的」的重要訊號
        await reviewCard(current.card_id, rating, Date.now() - shownAt);
        setQueue((q) => q.slice(1));
        setRevealed(false);
        setShownAt(Date.now());
        void studyStats().then(setStats).catch(() => undefined);
      } catch (e) {
        setError(errorMessage(e));
      }
    },
    [current, shownAt],
  );

  // 鍵盤操作：空白鍵翻牌，1~4 評分。背單字時手不該離開鍵盤。
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (!current || e.target instanceof HTMLInputElement) return;
      if (e.code === "Space") {
        e.preventDefault();
        setRevealed(true);
        return;
      }
      if (!revealed) return;
      const rating = Number(e.key);
      if (rating >= 1 && rating <= 4) void grade(rating as Rating);
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [current, revealed, grade]);

  async function onAddWord(e: FormEvent) {
    e.preventDefault();
    const word = newWord.trim();
    if (!word) return;
    try {
      await addWord(word);
      setNewWord("");
      await refresh();
    } catch (err) {
      setError(errorMessage(err));
    }
  }

  return (
    <>
      {stats && (
        <dl className="stats">
          <div>
            <dt>待複習</dt>
            <dd>{stats.due_now}</dd>
          </div>
          <div>
            <dt>今日新字</dt>
            <dd>{stats.new_today}</dd>
          </div>
          <div>
            <dt>已掌握</dt>
            <dd>{stats.known_words}</dd>
          </div>
          <div>
            <dt>學習中</dt>
            <dd>{stats.total_words}</dd>
          </div>
          <div>
            <dt>今日複習</dt>
            <dd>{stats.reviews_today}</dd>
          </div>
        </dl>
      )}

      {error && <p className="error">{error}</p>}

      {loading ? (
        <p className="muted">載入中…</p>
      ) : current ? (
        <section className="card">
          <p className="word">
            {current.word}
            <SpeakButton text={current.word} audioPath={current.audio_path} />
          </p>
          {current.ipa && <p className="ipa">{current.ipa}</p>}

          {revealed ? (
            <>
              <p className="meaning">{current.translation ?? current.gloss ?? "（尚無釋義）"}</p>
              <div className="ratings">
                <button onClick={() => grade(RATING.again)}>忘記了 (1)</button>
                <button onClick={() => grade(RATING.hard)}>有點難 (2)</button>
                <button onClick={() => grade(RATING.good)}>記得 (3)</button>
                <button onClick={() => grade(RATING.easy)}>很簡單 (4)</button>
              </div>
            </>
          ) : (
            <button className="reveal" onClick={() => setRevealed(true)}>
              顯示答案（空白鍵）
            </button>
          )}
        </section>
      ) : (
        <section className="done">
          <p>今天的份做完了 🎉</p>
          <p className="muted">
            每天固定引入少量新字才記得住。明天再回來，
            或到「牌組」頁把更多範圍排進來。
          </p>
        </section>
      )}

      <form className="add-word" onSubmit={onAddWord}>
        <input
          value={newWord}
          onChange={(e) => setNewWord(e.target.value)}
          placeholder="快速加入單字…"
          aria-label="快速加入單字"
        />
        <button type="submit">加入</button>
      </form>
    </>
  );
}

import { type FormEvent, useCallback, useEffect, useState } from "react";
import {
  addWord,
  buryCard,
  type CardView,
  errorMessage,
  listDueCards,
  RATING,
  type Rating,
  reviewCard,
  type QueueStatus,
  queueStatus,
  type StudyStats,
  studyStats,
  suspendCard,
  type WordDetail,
  wordDetail,
  dueSentences,
} from "../api";
import DueSentences from "../components/DueSentences";
import QueueEmpty from "../components/QueueEmpty";
import SpeakButton from "../components/SpeakButton";
import WordSenses from "../components/WordSenses";

/**
 * 複習頁：只做「看字 → 想意思 → 自評」這一條路徑。
 *
 * 閱讀理解、AI 對話、寫作批改會是各自獨立的頁面，
 * 但都建立在同一份卡片資料之上。
 */
export default function Review() {
  // 複習分兩種：單字卡，以及之前寫錯的翻譯句。
  //
  // 句子不跟單字卡混在同一個佇列裡：一個是「看字想意思」按四個鍵，
  // 另一個是打一整句話等模型批改，兩種節奏混在一起，兩邊都做不順。
  const [mode, setMode] = useState<"cards" | "sentences">("cards");
  const [sentenceCount, setSentenceCount] = useState(0);
  const [queue, setQueue] = useState<CardView[]>([]);
  const [stats, setStats] = useState<StudyStats | null>(null);
  const [status, setStatus] = useState<QueueStatus | null>(null);
  const [revealed, setRevealed] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [shownAt, setShownAt] = useState(() => Date.now());
  const [newWord, setNewWord] = useState("");
  // 翻面後要不要一併攤開整個詞條。
  //
  // 這個開關**刻意不隨著換卡重設**：想看細節的人每張都想看，
  // 每次都要再按一次等於沒有這個功能。收合狀態下也不會去查資料庫。
  const [showDetail, setShowDetail] = useState(false);
  const [detail, setDetail] = useState<WordDetail | null>(null);

  const refresh = useCallback(async () => {
    try {
      const [cards, s, q, sentences] = await Promise.all([
        listDueCards(),
        studyStats(),
        queueStatus(),
        dueSentences(),
      ]);
      setQueue(cards);
      setStats(s);
      setStatus(q);
      setSentenceCount(sentences.total);
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

  // 詳情是翻面之後才需要的，而且只在使用者真的展開時才查。
  // 沒翻面就先抓的話，等於每張卡都多打一次資料庫換來一份沒人看的資料。
  useEffect(() => {
    if (!current || !revealed || !showDetail) return;
    let cancelled = false;
    setDetail(null);
    void wordDetail(current.lemma_id)
      .then((d) => {
        if (!cancelled) setDetail(d);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [current, revealed, showDetail]);

  /**
   * 從佇列拿掉這張卡，但不評分。
   *
   * 埋葬與暫停都不該進 FSRS：使用者按的是「今天不想看到」或
   * 「這張根本不該出現」，不是「我忘記了」。把它們當成答錯會讓
   * 間隔被錯誤地縮短，越跳過越常出現，完全相反。
   */
  const skip = useCallback(
    async (action: (id: number) => Promise<boolean>) => {
      if (!current) return;
      try {
        await action(current.card_id);
        setQueue((q) => q.slice(1));
        setRevealed(false);
        setShownAt(Date.now());
        void refresh();
      } catch (e) {
        setError(errorMessage(e));
      }
    },
    [current, refresh],
  );

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

  // 鍵盤操作：空白鍵翻牌，1~4 評分，B 跳過、S 收起。
  // 背單字時手不該離開鍵盤。
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      // 在句子那一半按鍵不該動到單字卡。輸入框有焦點時本來就擋掉了，
      // 但送出之後焦點會落在「下一句」按鈕上——那時候按 B
      // 會把一張看不見的卡片跳過，而使用者根本不知道發生了什麼。
      if (mode === "sentences") return;
      if (!current || e.target instanceof HTMLInputElement) return;
      if (e.code === "Space") {
        e.preventDefault();
        setRevealed(true);
        return;
      }
      // 跳過與收起不必先翻牌：卡住的時候就是還沒想出來
      if (e.key === "b" || e.key === "B") {
        void skip(buryCard);
        return;
      }
      if (e.key === "s" || e.key === "S") {
        void skip(suspendCard);
        return;
      }
      if (!revealed) return;
      const rating = Number(e.key);
      if (rating >= 1 && rating <= 4) void grade(rating as Rating);
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [current, revealed, grade, skip, mode]);

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

      {/* 兩種複習擇一。句子那邊有東西時把數字標出來——不標的話
          使用者不會知道那裡有欠著的句子，而它們正是他寫錯過的 */}
      <div className="row review-modes">
        <button
          className={mode === "cards" ? "tab active" : "tab"}
          onClick={() => setMode("cards")}
        >
          單字{stats && stats.due_now + stats.new_today > 0 && `（${stats.due_now + stats.new_today}）`}
        </button>
        <button
          className={mode === "sentences" ? "tab active" : "tab"}
          onClick={() => setMode("sentences")}
        >
          句子{sentenceCount > 0 && `（${sentenceCount}）`}
        </button>
      </div>

      {error && <p className="error">{error}</p>}

      {mode === "sentences" ? (
        <DueSentences onGraded={refresh} />
      ) : loading ? (
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
              {/* 評分按鈕擺在完整詞條之前。自評要對著「剛剛考的那個意思」，
                  先看完十五條義項再按，按的就不是同一件事了。 */}
              <div className="ratings">
                <button onClick={() => grade(RATING.again)}>忘記了 (1)</button>
                <button onClick={() => grade(RATING.hard)}>有點難 (2)</button>
                <button onClick={() => grade(RATING.good)}>記得 (3)</button>
                <button onClick={() => grade(RATING.easy)}>很簡單 (4)</button>
              </div>

              <details
                className="card-detail"
                open={showDetail}
                onToggle={(e) => setShowDetail(e.currentTarget.open)}
              >
                <summary>完整詞條</summary>
                {detail ? (
                  <WordSenses detail={detail} />
                ) : (
                  <p className="muted">載入中…</p>
                )}
              </details>
            </>
          ) : (
            <button className="reveal" onClick={() => setRevealed(true)}>
              顯示答案（空白鍵）
            </button>
          )}

          <div className="card-actions">
            <button
              onClick={() => skip(buryCard)}
              title="今天不想看到這張，明天會自己回來。排程不受影響。"
            >
              跳過今天 (B)
            </button>
            <button
              onClick={() => skip(suspendCard)}
              title="這張不該出現。要到牌組頁主動恢復才會回來。"
            >
              收起這張 (S)
            </button>
          </div>
        </section>
      ) : status ? (
        <QueueEmpty status={status} onResume={refresh} />
      ) : null}

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

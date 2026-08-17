/**
 * 今天要重練的翻譯句，**一輪三句**。
 *
 * 這是翻譯題的「複習」那一半：做錯的句子不是留在紀錄裡等人翻出來，
 * 而是**明天自己回來**，直到寫對為止。規則刻意比 FSRS 簡單：
 *
 * ```text
 * 答錯 → 明天再出現
 * 答對 → 從此不再出現
 * 跳過 → 明天再出現，但不算答錯
 * ```
 *
 * ## 為什麼是三句，不是全部也不是一句
 *
 * 一開始是把今天到期的句子整批攤開來，十幾個空格一次出現在畫面上——
 * 那個數量本身就會讓人不想開始，而它們全都是他寫錯過的句子。
 *
 * 改成一次一句之後畫面舒服了，但**每一句都是一次完整的模型呼叫**：
 * 三句就是三次批改請求、三份 prompt，而它們完全可以擠在同一次裡。
 * 那時候複習還走 `regrade`，於是每一句又在那份練習的紀錄裡多一筆
 * 「第 N 次」——清單上的「做過 12 次」其實是複習了 12 句。
 *
 * 三句是兩邊的交集：看得完，也批得完。這一輪送完才抓下一輪。
 *
 * 一句每天只出現一次。當天反覆重寫同一句刷到全對，看起來是 100 分，
 * 實際上只是背下剛剛看到的參考答案——所以作答**之前**不顯示參考答案
 * 與上次的作答，那兩樣要送出之後才看得到。
 */
import { type FormEvent, useCallback, useEffect, useState } from "react";
import {
  currentLanguages,
  type DueSentence,
  type DueSentenceResult,
  dueSentences,
  errorMessage,
  exerciseLabels,
  gradeDueSentences,
  type ProfileLanguages,
  skipSentence,
} from "../api";
import Reference from "./Reference";

/** 一句的識別：跨練習時只有 (練習, 第幾題) 這一組指得出是哪一句。 */
const keyOf = (s: { exercise_id: number; item_index: number }) =>
  `${s.exercise_id}:${s.item_index}`;

export default function DueSentences({
  onGraded,
}: {
  /** 送出後統計與待練數量都會變，外面要跟著更新 */
  onGraded?: () => void;
}) {
  // 題型名稱要說得出「日文翻中文」，不能寫死英文
  const [langs, setLangs] = useState<ProfileLanguages>({ native: "zh-TW", target: "en" });
  const labels = exerciseLabels(langs);
  const [items, setItems] = useState<DueSentence[]>([]);
  // 今天總共還剩幾句（含這一輪）。一次只看得到三句的話，
  // 沒有這個數字就不知道還要練多久。
  const [total, setTotal] = useState(0);
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  // 這一輪的批改。有東西就代表已經送出去了，這時候不能再改答案。
  const [results, setResults] = useState<Record<string, DueSentenceResult>>({});
  const [submitted, setSubmitted] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // 今天練了幾句、對了幾句、跳過幾句。清單清空之後畫面只剩這個數字，
  // 沒有它的話練完就是一片空白。
  //
  // 跳過的要分開算：把它併進「沒寫對」會讓使用者以為自己寫錯了，
  // 而他根本沒寫。
  const [done, setDone] = useState({ passed: 0, missed: 0, skipped: 0 });

  const refresh = useCallback(async () => {
    try {
      const page = await dueSentences();
      setItems(page.items);
      setTotal(page.total);
      setError(null);
    } catch (e) {
      setError(errorMessage(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
    void currentLanguages()
      .then(setLangs)
      .catch(() => {});
  }, [refresh]);

  const answered = items.filter((s) => (drafts[keyOf(s)] ?? "").trim().length > 0);

  async function submit() {
    if (answered.length === 0) return;
    setBusy(true);
    setError(null);
    try {
      // 整輪一起送＝一次模型呼叫。沒作答的那幾句不送——空白會被
      // 批改成答錯，而他只是還沒寫到那一句，那一句今天還在。
      const graded = await gradeDueSentences(
        answered.map((s) => ({
          exercise_id: s.exercise_id,
          item_index: s.item_index,
          answer: (drafts[keyOf(s)] ?? "").trim(),
        })),
      );

      const byKey: Record<string, DueSentenceResult> = {};
      for (const r of graded) byKey[keyOf(r)] = r;
      setResults(byKey);
      setSubmitted(true);

      const passed = graded.filter((r) => r.correct).length;
      setDone((d) => ({
        ...d,
        passed: d.passed + passed,
        missed: d.missed + (graded.length - passed),
      }));
      setTotal((t) => Math.max(0, t - graded.length));
      onGraded?.();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  /**
   * 今天不寫這一句。
   *
   * 明天照樣回來，而且不算答錯——寫不出來跟寫錯是兩件事，混在一起的話
   * 「錯過 5 次」會出現在一個他一次都沒寫過的句子上。這也是為什麼
   * 這裡不送出空白答案：那會被批改成答錯。
   *
   * 只把那一句從畫面上拿掉，不重抓整輪：旁邊兩句可能寫到一半了。
   */
  async function skip(sentence: DueSentence) {
    setBusy(true);
    setError(null);
    try {
      await skipSentence(sentence.exercise_id, sentence.item_index);
      const remaining = items.filter((s) => keyOf(s) !== keyOf(sentence));
      setItems(remaining);
      setDone((d) => ({ ...d, skipped: d.skipped + 1 }));
      setTotal((t) => Math.max(0, t - 1));
      onGraded?.();
      // 整輪都跳過時要接上下一輪。不接的話畫面會說「今天的句子都看過了」，
      // 而今天其實還有一整排——空的原因是這一輪被跳完了，不是沒有句子。
      if (remaining.length === 0) await refresh();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  async function next() {
    setResults({});
    setSubmitted(false);
    setDrafts({});
    setBusy(true);
    await refresh();
    setBusy(false);
  }

  function onSubmit(e: FormEvent) {
    e.preventDefault();
    if (busy) return;
    if (submitted) void next();
    else void submit();
  }

  if (items.length === 0) {
    // 送出之後清空是常態，那時候要說得出「剛剛練完了」而不是一片空白
    if (done.passed + done.missed + done.skipped === 0) return null;
    // 明天回來的有兩種，而且要分得出來：寫錯的是他寫了但不對，
    // 跳過的是他今天不想寫。都說成「錯了」不誠實。
    const returning = [
      done.missed > 0 && `${done.missed} 句沒寫對`,
      done.skipped > 0 && `${done.skipped} 句跳過`,
    ].filter(Boolean);
    return (
      <section className="panel">
        <h2>今天的句子</h2>
        <p className="ok">今天的句子都看過了。</p>
        {returning.length > 0 && (
          <p className="muted">明天會再出現：{returning.join("、")}。</p>
        )}
      </section>
    );
  }

  return (
    <section className="panel exercise">
      {/* 「還有幾句」含這一輪，送出後就少掉這一輪的份 */}
      <h2>今天的句子{total > 0 && `（還有 ${total} 句）`}</h2>
      <p className="muted">
        這些是之前寫錯的句子，隔一天再想一次。寫對就不會再出現，
        寫錯或今天跳過的明天回來。
      </p>

      <form className="due-sentences" onSubmit={onSubmit}>
        {items.map((s, i) => {
          const key = keyOf(s);
          const result = results[key];
          return (
            <div key={key} className="question due-sentence">
              <p className="prompt">
                {i + 1}.
                <span className="tag">{labels[s.kind] ?? s.kind}</span>
                {s.source}
                {s.target_word && (
                  <span className="tag" title="這題想讓你用到的字">
                    {s.target_word}
                  </span>
                )}
                {s.misses > 1 && <span className="muted">錯過 {s.misses} 次</span>}
              </p>

              <div className="row due-sentence-row">
                <input
                  value={drafts[key] ?? ""}
                  onChange={(e) => setDrafts({ ...drafts, [key]: e.target.value })}
                  placeholder="翻譯這一句"
                  aria-label={`翻譯：${s.source}`}
                  // 批改完就不能再改：那份答案已經送出去、也已經算進排程了
                  disabled={busy || submitted}
                  autoFocus={i === 0}
                />
                {!submitted && (
                  // `type="button"` 不能少：form 裡的按鈕預設是 submit，
                  // 少了它按跳過會變成把整輪送出去
                  <button
                    type="button"
                    onClick={() => void skip(s)}
                    disabled={busy}
                    title="今天先不寫這一句。明天會再出現，不算答錯。"
                  >
                    跳過
                  </button>
                )}
              </div>

              {result && (
                <p className={result.correct ? "ok" : "error"}>
                  {result.correct ? "✓" : "✗"}{" "}
                  <Reference reference={result.reference} formal={result.reference_formal} />
                  {result.comment && <span>　{result.comment}</span>}
                </p>
              )}
              {submitted && !result && (
                <p className="muted">
                  這次的批改沒有講到這一句。它會照「還沒寫對」處理，明天再出現一次。
                </p>
              )}
            </div>
          );
        })}

        {error && <p className="error">{error}</p>}

        <div className="row submit-row">
          {submitted ? (
            <button className="primary" type="submit" disabled={busy}>
              {total > 0 ? "下一輪" : "完成"}
            </button>
          ) : (
            <button className="primary" type="submit" disabled={busy || answered.length === 0}>
              {busy ? "批改中…" : `送出 ${answered.length} 句`}
            </button>
          )}
          {!submitted && answered.length < items.length && (
            <span className="muted">
              沒寫的那 {items.length - answered.length} 句今天還會再出現
            </span>
          )}
        </div>
      </form>

      {done.passed + done.missed + done.skipped > 0 && (
        <p className="muted">
          今天到目前：
          {[
            `${done.passed} 句寫對`,
            done.missed > 0 && `${done.missed} 句明天再練`,
            done.skipped > 0 && `${done.skipped} 句跳過`,
          ]
            .filter(Boolean)
            .join("、")}
          。
        </p>
      )}
    </section>
  );
}

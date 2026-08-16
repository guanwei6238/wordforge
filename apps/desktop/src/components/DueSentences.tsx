/**
 * 今天要重練的翻譯句。
 *
 * 這是翻譯題的「複習」那一半：做錯的句子不是留在紀錄裡等人翻出來，
 * 而是**明天自己回來**，直到寫對為止。規則刻意比 FSRS 簡單：
 *
 * ```text
 * 答錯 → 明天再出現
 * 答對 → 從此不再出現
 * ```
 *
 * 一句每天只出現一次。當天反覆重寫同一句刷到全對，看起來是 100 分，
 * 實際上只是背下剛剛看到的參考答案——所以這裡也**不顯示**參考答案
 * 與上次的作答，那兩樣要送出之後才看得到。
 */
import { useCallback, useEffect, useState } from "react";
import {
  currentLanguages,
  type DueSentence,
  dueSentences,
  errorMessage,
  exerciseLabels,
  type Feedback,
  type ProfileLanguages,
  regradeItems,
} from "../api";

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
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // 送出後那一輪的結果：哪幾句對了、哪幾句還沒
  const [result, setResult] = useState<{ passed: number; missed: number } | null>(null);

  const refresh = useCallback(async () => {
    try {
      setItems(await dueSentences());
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

  const key = (s: DueSentence) => `${s.exercise_id}:${s.item_index}`;
  const answered = items.filter((s) => (drafts[key(s)] ?? "").trim().length > 0);

  async function submit() {
    setBusy(true);
    setError(null);
    setResult(null);
    try {
      // 同一份練習的句子要一起送：批改是以「一份練習」為單位合併回去的，
      // 一句一次呼叫等於同一份練習被批改好幾次，每次都是一趟完整的模型呼叫
      const byExercise = new Map<number, { index: number; answer: string }[]>();
      for (const s of answered) {
        const list = byExercise.get(s.exercise_id) ?? [];
        list.push({ index: s.item_index, answer: (drafts[key(s)] ?? "").trim() });
        byExercise.set(s.exercise_id, list);
      }

      let passed = 0;
      let missed = 0;
      for (const [exerciseId, redone] of byExercise) {
        const feedback: Feedback = await regradeItems(exerciseId, redone);
        for (const { index } of redone) {
          if (feedback.items?.[index]?.correct) passed += 1;
          else missed += 1;
        }
      }

      setResult({ passed, missed });
      setDrafts({});
      await refresh();
      onGraded?.();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  if (items.length === 0) {
    // 送出之後清空是常態，那時候要說得出「剛剛練完了」而不是一片空白
    if (!result) return null;
    return (
      <section className="panel">
        <h2>今天的句子</h2>
        <p className="ok">
          今天的句子都練完了{result.missed > 0 && `，其中 ${result.missed} 句明天會再出現`}。
        </p>
      </section>
    );
  }

  return (
    <section className="panel">
      <h2>今天的句子（{items.length}）</h2>
      <p className="muted">
        這些是之前寫錯的句子，隔一天再想一次。寫對就不會再出現，寫錯的明天回來。
      </p>

      {items.map((s) => (
        <label key={key(s)} className="retry-item">
          <span className="attempt-question">
            <span className="tag">{labels[s.kind] ?? s.kind}</span>
            {s.source}
            {s.target_word && <span className="tag">{s.target_word}</span>}
            {s.misses > 1 && <span className="muted">　錯過 {s.misses} 次</span>}
          </span>
          <textarea
            rows={2}
            value={drafts[key(s)] ?? ""}
            placeholder="翻譯這一句"
            disabled={busy}
            onChange={(e) => setDrafts({ ...drafts, [key(s)]: e.target.value })}
          />
        </label>
      ))}

      {error && <p className="error">{error}</p>}
      {result && (
        <p className="muted">
          上一輪：{result.passed} 句寫對
          {result.missed > 0 && `，${result.missed} 句明天再練`}。
        </p>
      )}

      <button
        className="primary"
        onClick={() => void submit()}
        disabled={busy || answered.length === 0}
      >
        {busy ? "批改中…" : `送出 ${answered.length} 句`}
      </button>
    </section>
  );
}

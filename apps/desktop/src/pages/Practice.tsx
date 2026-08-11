import { useCallback, useEffect, useState } from "react";
import {
  currentLanguages,
  errorMessage,
  exerciseLabels,
  type ExerciseKind,
  type ExerciseView,
  type Feedback,
  generateExercise,
  gradeExercise,
  languageName,
  listMaterials,
  type Material,
  practiceStatus,
  type PracticeStatus,
  type ProfileLanguages,
} from "../api";
import LlmSetup from "../components/LlmSetup";
import SpeakButton from "../components/SpeakButton";

/**
 * AI 練習頁。
 *
 * 一整條迴圈都在這裡：依程度出題 → 作答 → 批改 → 不會的字自動排進複習。
 * 閱讀測驗時可以點文章裡的任何字標記「我不會」，那些字也會一起進牌組。
 */
export default function Practice() {
  const [status, setStatus] = useState<PracticeStatus | null>(null);
  const [kind, setKind] = useState<ExerciseKind | "auto">("auto");
  const [exercise, setExercise] = useState<ExerciseView | null>(null);
  const [answers, setAnswers] = useState<string[]>([]);
  const [choices, setChoices] = useState<(number | null)[]>([]);
  const [marked, setMarked] = useState<string[]>([]);
  const [feedback, setFeedback] = useState<Feedback | null>(null);
  const [busy, setBusy] = useState<"generating" | "grading" | null>(null);
  const [error, setError] = useState<string | null>(null);
  // 題型名稱要說得出「日文翻中文」，不能寫死英文
  const [langs, setLangs] = useState<ProfileLanguages>({ native: "zh-TW", target: "en" });
  const labels = exerciseLabels(langs);
  // 指定教材後，模型只能從那本書取材
  const [materials, setMaterials] = useState<Material[]>([]);
  const [materialId, setMaterialId] = useState<number | null>(null);

  const refresh = useCallback(async () => {
    try {
      setStatus(await practiceStatus());
    } catch (e) {
      setError(errorMessage(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
    void currentLanguages().then(setLangs).catch(() => {});
    void listMaterials()
      .then(setMaterials)
      .catch(() => {});
  }, [refresh]);

  async function start() {
    setBusy("generating");
    setError(null);
    setFeedback(null);
    setMarked([]);
    try {
      const ex = await generateExercise(kind, materialId);
      setExercise(ex);
      setAnswers(ex.body.kind === "translation" ? ex.body.items.map(() => "") : []);
      setChoices(
        ex.body.kind === "reading"
          ? ex.body.questions.map(() => null)
          : ex.body.kind === "choices"
            ? ex.body.items.map(() => null)
            : [],
      );
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  }

  async function submit() {
    if (!exercise) return;
    setBusy("grading");
    setError(null);
    try {
      setFeedback(
        await gradeExercise({
          exercise_id: exercise.exercise_id,
          answers,
          choices,
          marked_unknown: marked,
        }),
      );
      await refresh();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  }

  /** 點文章裡的字標記「我不會」。再點一次取消。 */
  function toggleMarked(word: string) {
    const clean = word.replace(/[^\p{L}\p{N}'-]/gu, "");
    if (!clean) return;
    setMarked((m) =>
      m.some((w) => w.toLowerCase() === clean.toLowerCase())
        ? m.filter((w) => w.toLowerCase() !== clean.toLowerCase())
        : [...m, clean],
    );
  }

  if (status && !status.llm_ready) {
    return (
      <div className="practice">
        <section className="panel note">
          <h2>先設定 AI 後端</h2>
          <p className="muted">
            出題與批改需要模型。如果你已經有 Claude 或 ChatGPT 訂閱，
            直接用本機的 CLI 就好，不必再開一份 API 帳單。
          </p>
        </section>
        <LlmSetup onChanged={refresh} />
      </div>
    );
  }

  return (
    <div className="practice">
      {status && (
        <section className="panel">
          <div className="row">
            <label>
              題型
              <select
                value={kind}
                onChange={(e) => setKind(e.target.value as ExerciseKind | "auto")}
                disabled={busy !== null}
              >
                <option value="auto">
                  自動（依程度：{labels[status.recommended]}）
                </option>
                {status.requirements.map(([k, need]) => (
                  <option key={k} value={k} disabled={status.vocabulary < need}>
                    {labels[k]}
                    {status.vocabulary < need ? `（需要 ${need} 字）` : ""}
                  </option>
                ))}
              </select>
            </label>
            {materials.length > 0 && (
              <label>
                取材範圍
                <select
                  value={materialId ?? ""}
                  onChange={(e) => setMaterialId(e.target.value ? Number(e.target.value) : null)}
                  disabled={busy !== null}
                >
                  <option value="">自由出題</option>
                  {materials.map((m) => (
                    <option key={m.id} value={m.id}>
                      只從《{m.title}》出題
                    </option>
                  ))}
                </select>
              </label>
            )}
            <button className="primary" onClick={start} disabled={busy !== null}>
              {busy === "generating" ? "出題中…" : exercise ? "換一題" : "開始練習"}
            </button>
          </div>
          <p className="muted hint">
            詞彙量約 {status.vocabulary.toLocaleString()} 字
            {status.weak_grammar.length > 0 &&
              `　·　最近常錯：${status.weak_grammar.join("、")}`}
          </p>
          {busy === "generating" && (
            <p className="muted">
              模型正在出題，用 CLI 的話可能要幾十秒。
            </p>
          )}
        </section>
      )}

      {error && <p className="error">{error}</p>}

      {exercise && (
        <section className="panel exercise">
          {exercise.body.kind === "translation" && (
            <>
              <h2>
                {exercise.body.to_target
                  ? `翻成${languageName(langs.target)}`
                  : `翻成${languageName(langs.native)}`}
              </h2>
              {exercise.body.items.map((item, i) => (
                <div key={i} className="question">
                  <p className="prompt">
                    {i + 1}. {item.source}
                    {item.target_word && (
                      <span className="tag" title="這題想讓你用到的字">
                        {item.target_word}
                      </span>
                    )}
                  </p>
                  <input
                    value={answers[i] ?? ""}
                    onChange={(e) =>
                      setAnswers((a) => a.map((v, j) => (j === i ? e.target.value : v)))
                    }
                    disabled={feedback !== null}
                    placeholder="你的翻譯…"
                  />
                  {feedback?.items[i] && (
                    <p className={feedback.items[i].correct ? "ok" : "error"}>
                      {feedback.items[i].correct ? "✓" : "✗"}{" "}
                      {feedback.items[i].reference && (
                        <span className="muted">參考：{feedback.items[i].reference}　</span>
                      )}
                      {feedback.items[i].comment}
                    </p>
                  )}
                </div>
              ))}
            </>
          )}

          {exercise.body.kind === "reading" && (
            <>
              <h2>{exercise.body.title}</h2>
              {exercise.coverage != null && (
                <p className="muted hint">
                  這篇文章有 {Math.round(exercise.coverage * 100)}% 的字你已經學過。
                  點任何一個字可以標記「我不會」，送出後會排進複習。
                </p>
              )}
              <p className="passage">
                {exercise.body.passage.split(/(\s+)/).map((chunk, i) =>
                  chunk.trim() ? (
                    <span
                      key={i}
                      className={
                        marked.some(
                          (w) =>
                            w.toLowerCase() ===
                            chunk.replace(/[^\p{L}\p{N}'-]/gu, "").toLowerCase(),
                        )
                          ? "word marked"
                          : "word"
                      }
                      onClick={() => toggleMarked(chunk)}
                    >
                      {chunk}
                    </span>
                  ) : (
                    chunk
                  ),
                )}
              </p>

              {exercise.body.new_words.length > 0 && (
                <ul className="new-words">
                  {exercise.body.new_words.map((w) => (
                    <li key={w.word}>
                      <strong>{w.word}</strong>
                      <SpeakButton text={w.word} />
                      <span className="muted"> {w.gloss}</span>
                    </li>
                  ))}
                </ul>
              )}

              <Choices
                items={exercise.body.questions}
                choices={choices}
                setChoices={setChoices}
                feedback={feedback}
              />
            </>
          )}

          {exercise.body.kind === "choices" && (
            <>
              <h2>文法練習</h2>
              <Choices
                items={exercise.body.items}
                choices={choices}
                setChoices={setChoices}
                feedback={feedback}
              />
            </>
          )}

          {marked.length > 0 && !feedback && (
            <p className="muted">
              標記為不會的字：{marked.join("、")}
            </p>
          )}

          {!feedback && (
            <div className="row">
              <button className="primary" onClick={submit} disabled={busy !== null}>
                {busy === "grading" ? "批改中…" : "送出"}
              </button>
            </div>
          )}
        </section>
      )}

      {feedback && (
        <section className="panel">
          <h2>
            批改結果
            {feedback.score != null && <span className="score"> {Math.round(feedback.score)} 分</span>}
          </h2>

          {feedback.corrections.length > 0 && (
            <ul className="corrections">
              {feedback.corrections.map((c, i) => (
                <li key={i}>
                  <span className="wrong">{c.original}</span>
                  {" → "}
                  <span className="right">{c.corrected}</span>
                  {c.grammar_point && <span className="tag">{c.grammar_point}</span>}
                  {c.explanation && <p className="muted">{c.explanation}</p>}
                </li>
              ))}
            </ul>
          )}

          {feedback.glossary?.length > 0 && (
            <div className="glossary">
              <h3>
                文章解析
                <span className="muted"> · 來自你匯入的字典，不是 AI 寫的</span>
              </h3>
              <dl>
                {feedback.glossary.map((g, i) => (
                  <div key={i} className="gloss-row">
                    <dt>
                      {g.text}
                      {g.is_phrase && (
                        <span className="tag" title="片語：單看每個字查不出這個意思">
                          片語
                        </span>
                      )}
                    </dt>
                    <dd>{g.translation ?? g.gloss ?? <span className="muted">查無釋義</span>}</dd>
                  </div>
                ))}
              </dl>
            </div>
          )}

          {feedback.added_to_deck.length > 0 ? (
            <p className="ok">
              已把 {feedback.added_to_deck.length} 個你不熟的字排進複習：
              {feedback.added_to_deck.join("、")}
            </p>
          ) : (
            feedback.unknown_words.length > 0 && (
              <p className="muted">
                判斷你不熟的字：{feedback.unknown_words.join("、")}
                （都已經在牌組裡了）
              </p>
            )
          )}

          <div className="row">
            {materials.length > 0 && (
              <label>
                取材範圍
                <select
                  value={materialId ?? ""}
                  onChange={(e) => setMaterialId(e.target.value ? Number(e.target.value) : null)}
                  disabled={busy !== null}
                >
                  <option value="">自由出題</option>
                  {materials.map((m) => (
                    <option key={m.id} value={m.id}>
                      只從《{m.title}》出題
                    </option>
                  ))}
                </select>
              </label>
            )}
            <button className="primary" onClick={start} disabled={busy !== null}>
              下一題
            </button>
          </div>
        </section>
      )}
    </div>
  );
}

/** 選擇題，閱讀測驗與文法練習共用。 */
function Choices({
  items,
  choices,
  setChoices,
  feedback,
}: {
  items: { question: string; options: string[]; answer_index: number; explanation: string | null }[];
  choices: (number | null)[];
  setChoices: (fn: (c: (number | null)[]) => (number | null)[]) => void;
  feedback: Feedback | null;
}) {
  return (
    <ol className="questions">
      {items.map((q, i) => (
        <li key={i}>
          <p className="prompt">{q.question}</p>
          <div className="options">
            {q.options.map((option, j) => (
              <label
                key={j}
                className={
                  feedback ? (j === q.answer_index ? "option right" : "option") : "option"
                }
              >
                <input
                  type="radio"
                  name={`q${i}`}
                  checked={choices[i] === j}
                  disabled={feedback !== null}
                  onChange={() => setChoices((c) => c.map((v, k) => (k === i ? j : v)))}
                />
                {option}
              </label>
            ))}
          </div>
          {feedback && q.explanation && <p className="muted">{q.explanation}</p>}
        </li>
      ))}
    </ol>
  );
}

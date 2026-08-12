import { useCallback, useEffect, useMemo, useState } from "react";
import {
  currentLanguages,
  DIFFICULTY_LABELS,
  errorMessage,
  exerciseLabels,
  type ExerciseKind,
  type ExerciseSummary,
  type ExerciseView,
  type Feedback,
  generateExercise,
  type GlossaryNote,
  getStudySettings,
  gradeExercise,
  languageName,
  listExercises,
  listMaterials,
  loadExercise,
  type Material,
  practiceStatus,
  type PracticeStatus,
  type ProfileLanguages,
  updateStudySettings,
} from "../api";
import LlmSetup from "../components/LlmSetup";
import SpeakButton from "../components/SpeakButton";

/** 一步調幾 px。太細要按很多次，太粗會跳過剛好的那一級。 */
const FONT_STEP = 2;
const FONT_MIN = 12;
const FONT_MAX = 32;

/**
 * AI 練習頁。
 *
 * 一整條迴圈都在這裡：依程度出題 → 作答 → 批改 → 不會的字自動排進複習。
 *
 * 閱讀測驗分兩個階段，點文章的意思也跟著換：
 *
 * - **作答前**：點任何一個字＝標記「我不會」，送出時一起排進複習。
 * - **解析時**：點任何一個字＝查它的釋義。這時候才給翻譯，
 *   作答前給等於直接送答案。
 */
export default function Practice() {
  const [status, setStatus] = useState<PracticeStatus | null>(null);
  const [kind, setKind] = useState<ExerciseKind | "auto">("auto");
  const [exercise, setExercise] = useState<ExerciseView | null>(null);
  const [answers, setAnswers] = useState<string[]>([]);
  const [choices, setChoices] = useState<(number | null)[]>([]);
  const [marked, setMarked] = useState<string[]>([]);
  const [feedback, setFeedback] = useState<Feedback | null>(null);
  const [busy, setBusy] = useState<"generating" | "grading" | "loading" | null>(null);
  const [error, setError] = useState<string | null>(null);
  // 題型名稱要說得出「日文翻中文」，不能寫死英文
  const [langs, setLangs] = useState<ProfileLanguages>({ native: "zh-TW", target: "en" });
  const labels = exerciseLabels(langs);
  // 指定教材後，模型只能從那本書取材
  const [materials, setMaterials] = useState<Material[]>([]);
  const [materialId, setMaterialId] = useState<number | null>(null);
  // 文章字級。存在 profile 裡，換一台電腦也還在
  const [fontSize, setFontSize] = useState(18);
  // 練習紀錄。做過的題目可以整份叫回來重做
  const [history, setHistory] = useState<ExerciseSummary[]>([]);
  const [showHistory, setShowHistory] = useState(false);
  // 解析階段點到的字，顯示釋義用
  const [lookup, setLookup] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setStatus(await practiceStatus());
    } catch (e) {
      setError(errorMessage(e));
    }
  }, []);

  const refreshHistory = useCallback(async () => {
    try {
      setHistory(await listExercises());
    } catch (e) {
      setError(errorMessage(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
    void refreshHistory();
    void currentLanguages().then(setLangs).catch(() => {});
    void listMaterials()
      .then(setMaterials)
      .catch(() => {});
    void getStudySettings()
      .then((s) => setFontSize(s.reading_font_size))
      .catch(() => {});
  }, [refresh, refreshHistory]);

  /** 把一份題目擺上畫面，順便把上一份的作答與批改清乾淨。 */
  function present(ex: ExerciseView | null) {
    setExercise(ex);
    setFeedback(null);
    setMarked([]);
    setLookup(null);
    setAnswers(ex?.body.kind === "translation" ? ex.body.items.map(() => "") : []);
    setChoices(
      ex?.body.kind === "reading"
        ? ex.body.questions.map(() => null)
        : ex?.body.kind === "choices"
          ? ex.body.items.map(() => null)
          : [],
    );
  }

  async function start() {
    // 先清空。留著上一題的話，等模型的那幾十秒裡畫面顯示的是
    // 已經作廢的內容，而且捲到一半送出還會送到舊的 exercise_id。
    present(null);
    setBusy("generating");
    setError(null);
    try {
      present(await generateExercise(kind, materialId));
      await refreshHistory();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  }

  /** 從紀錄叫回一份做過的題目，原封不動再做一次。 */
  async function redo(exerciseId: number) {
    present(null);
    setBusy("loading");
    setError(null);
    try {
      present(await loadExercise(exerciseId));
      setShowHistory(false);
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
      await refreshHistory();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  }

  async function changeFontSize(next: number) {
    const clamped = Math.min(FONT_MAX, Math.max(FONT_MIN, next));
    setFontSize(clamped);
    try {
      // 後端會夾住範圍，用回傳值覆蓋才是真的存下來的
      const stored = await updateStudySettings({
        ...(await getStudySettings()),
        reading_font_size: clamped,
      });
      setFontSize(stored.reading_font_size);
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  /** 點文章裡的字標記「我不會」。再點一次取消。 */
  function toggleMarked(word: string) {
    const clean = normalizeWord(word);
    if (!clean) return;
    setMarked((m) =>
      m.some((w) => w.toLowerCase() === clean)
        ? m.filter((w) => w.toLowerCase() !== clean)
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

  const reading = exercise?.body.kind === "reading" ? exercise.body : null;

  return (
    <div className={reading ? "practice wide" : "practice"}>
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
              <MaterialPicker
                materials={materials}
                value={materialId}
                onChange={setMaterialId}
                disabled={busy !== null}
              />
            )}
            <button className="primary" onClick={start} disabled={busy !== null}>
              {busy === "generating" ? "出題中…" : exercise ? "換一題" : "開始練習"}
            </button>
            <button onClick={() => setShowHistory((v) => !v)} disabled={busy !== null}>
              練習紀錄{history.length > 0 ? `（${history.length}）` : ""}
            </button>
          </div>
          <p className="muted hint">
            詞彙量約 {status.vocabulary.toLocaleString()} 字
            {status.weak_grammar.length > 0 &&
              `　·　最近常錯：${status.weak_grammar.join("、")}`}
          </p>
          {busy === "generating" && (
            <p className="muted busy">
              <span className="spinner" aria-hidden="true" />
              模型正在出題，用 CLI 的話可能要幾十秒。
            </p>
          )}
          {busy === "loading" && (
            <p className="muted busy">
              <span className="spinner" aria-hidden="true" />
              正在取回那份練習…
            </p>
          )}
        </section>
      )}

      {showHistory && (
        <History
          items={history}
          labels={labels}
          disabled={busy !== null}
          onRedo={redo}
        />
      )}

      {error && <p className="error">{error}</p>}

      {/* 閱讀測驗：文章在左、題目與解析在右。
          兩欄是刻意的——對照文章回答本來就要來回看，
          上下排的話每答一題都要捲回去。 */}
      {exercise && reading && (
        <div className="reading-layout">
          <section className="panel exercise passage-pane">
            <div className="row title-row">
              <h2>{reading.title}</h2>
              <span className="font-size">
                <button
                  onClick={() => changeFontSize(fontSize - FONT_STEP)}
                  disabled={fontSize <= FONT_MIN}
                  title="縮小文章字級"
                >
                  A−
                </button>
                <span className="muted">{fontSize}px</span>
                <button
                  onClick={() => changeFontSize(fontSize + FONT_STEP)}
                  disabled={fontSize >= FONT_MAX}
                  title="放大文章字級"
                >
                  A+
                </button>
              </span>
            </div>

            <p className="muted hint">
              {feedback
                ? "點文章裡的任何一個字可以查它的意思。"
                : "點任何一個字可以標記「我不會」，送出後會排進複習。"}
              {exercise.coverage != null &&
                `　這篇有 ${Math.round(exercise.coverage * 100)}% 的字你已經學過。`}
            </p>

            <p className="passage" style={{ fontSize: `${fontSize}px` }}>
              {reading.passage.split(/(\s+)/).map((chunk, i) => {
                if (!chunk.trim()) return chunk;
                const word = normalizeWord(chunk);
                const isMarked = marked.some((w) => w.toLowerCase() === word);
                return (
                  <span
                    key={i}
                    className={[
                      "word",
                      isMarked ? "marked" : "",
                      feedback && lookup === word ? "looking" : "",
                    ]
                      .filter(Boolean)
                      .join(" ")}
                    onClick={() =>
                      feedback
                        ? setLookup((cur) => (cur === word ? null : word))
                        : toggleMarked(chunk)
                    }
                  >
                    {chunk}
                  </span>
                );
              })}
            </p>

            {feedback && lookup && (
              <WordNotes
                term={lookup}
                glossary={feedback.glossary ?? []}
                onClose={() => setLookup(null)}
              />
            )}

            {marked.length > 0 && !feedback && (
              <p className="muted">標記為不會的字：{marked.join("、")}</p>
            )}

            {reading.new_words.length > 0 && (
              <ul className="new-words">
                {reading.new_words.map((w) => (
                  <li key={w.word}>
                    <strong>{w.word}</strong>
                    <SpeakButton text={w.word} />
                    <span className="muted"> {w.gloss}</span>
                  </li>
                ))}
              </ul>
            )}

            {/* 全文翻譯只在解析時給，而且要自己展開——
                作答前給等於直接送答案，一打開就攤平則會讓人先看翻譯再讀原文 */}
            {feedback && reading.translation && (
              <details className="full-translation">
                <summary>全文翻譯</summary>
                <p>{reading.translation}</p>
              </details>
            )}
          </section>

          <div className="answer-pane">
            <section className="panel exercise">
              <Choices
                items={reading.questions}
                choices={choices}
                setChoices={setChoices}
                feedback={feedback}
              />
              {!feedback && (
                <button className="primary" onClick={submit} disabled={busy !== null}>
                  {busy === "grading" ? "批改中…" : "送出"}
                </button>
              )}
            </section>
            {feedback && (
              <FeedbackPanel
                feedback={feedback}
                materials={materials}
                materialId={materialId}
                setMaterialId={setMaterialId}
                busy={busy}
                onNext={start}
              />
            )}
          </div>
        </div>
      )}

      {exercise && !reading && (
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

          {!feedback && (
            <div className="row">
              <button className="primary" onClick={submit} disabled={busy !== null}>
                {busy === "grading" ? "批改中…" : "送出"}
              </button>
            </div>
          )}
        </section>
      )}

      {feedback && !reading && (
        <FeedbackPanel
          feedback={feedback}
          materials={materials}
          materialId={materialId}
          setMaterialId={setMaterialId}
          busy={busy}
          onNext={start}
        />
      )}
    </div>
  );
}

/** 跟後端 `wordforge_core::text::normalize` 對得上的比對鍵。 */
function normalizeWord(raw: string): string {
  return raw.replace(/[^\p{L}\p{N}'-]/gu, "").toLowerCase();
}

/** 點到的那個字的釋義，以及包含它的片語。 */
function WordNotes({
  term,
  glossary,
  onClose,
}: {
  term: string;
  glossary: GlossaryNote[];
  onClose: () => void;
}) {
  // 片語也要跳出來：`search for` 分開查兩個字都得不到「尋找」
  const hits = useMemo(
    () =>
      glossary.filter(
        (g) => g.term === term || (g.is_phrase && g.term.split(/\s+/).includes(term)),
      ),
    [glossary, term],
  );

  return (
    <div className="word-notes">
      <div className="row title-row">
        <strong>{term}</strong>
        <button onClick={onClose} title="關閉">
          ✕
        </button>
      </div>
      {hits.length === 0 ? (
        <p className="muted">
          你匯入的字典裡沒有這個詞條。換一份涵蓋更廣的字典就查得到。
        </p>
      ) : (
        <dl>
          {hits.map((g, i) => (
            <div key={i} className="gloss-row">
              <dt>
                {g.text}
                {g.is_phrase && (
                  <span className="tag" title="片語：單看每個字查不出這個意思">
                    片語
                  </span>
                )}
                <SpeakButton text={g.text} />
              </dt>
              <dd>{g.translation ?? g.gloss ?? <span className="muted">查無釋義</span>}</dd>
            </div>
          ))}
        </dl>
      )}
    </div>
  );
}

/** 練習紀錄。做過的題目整份叫回來重做，批改照常走一次。 */
function History({
  items,
  labels,
  disabled,
  onRedo,
}: {
  items: ExerciseSummary[];
  labels: Record<ExerciseKind, string>;
  disabled: boolean;
  onRedo: (id: number) => void;
}) {
  if (items.length === 0) {
    return (
      <section className="panel">
        <h2>練習紀錄</h2>
        <p className="muted">還沒有做過練習。做完的題目會留在這裡，可以整份再做一次。</p>
      </section>
    );
  }

  return (
    <section className="panel">
      <h2>練習紀錄</h2>
      <ul className="history">
        {items.map((it) => (
          <li key={it.exercise_id}>
            <div>
              <span className="tag">{labels[it.kind] ?? it.kind}</span>
              <span className="history-title">{it.title}</span>
            </div>
            <div className="muted history-meta">
              {formatWhen(it.created_at)}
              {it.score != null && `　·　${Math.round(it.score)} 分`}
              {it.score == null && "　·　沒作答"}
              {it.coverage != null && `　·　覆蓋率 ${Math.round(it.coverage * 100)}%`}
            </div>
            <button onClick={() => onRedo(it.exercise_id)} disabled={disabled}>
              再做一次
            </button>
          </li>
        ))}
      </ul>
      <p className="muted hint">
        重做的是同一份題目，不會再花一次出題的額度；送出後一樣由模型批改，
        舊的那次批改也留著。
      </p>
    </section>
  );
}

/** 資料庫存的是 RFC 3339 的 UTC，顯示成本地時間。 */
function formatWhen(iso: string): string {
  const d = new Date(iso.endsWith("Z") ? iso : `${iso}Z`);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString(undefined, {
    month: "numeric",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function MaterialPicker({
  materials,
  value,
  onChange,
  disabled,
}: {
  materials: Material[];
  value: number | null;
  onChange: (id: number | null) => void;
  disabled: boolean;
}) {
  return (
    <label>
      取材範圍
      <select
        value={value ?? ""}
        onChange={(e) => onChange(e.target.value ? Number(e.target.value) : null)}
        disabled={disabled}
      >
        <option value="">自由出題</option>
        {materials.map((m) => (
          <option key={m.id} value={m.id}>
            只從《{m.title}》出題
          </option>
        ))}
      </select>
    </label>
  );
}

/** 批改結果。閱讀題擺在右欄，其他題型擺在下面。 */
function FeedbackPanel({
  feedback,
  materials,
  materialId,
  setMaterialId,
  busy,
  onNext,
}: {
  feedback: Feedback;
  materials: Material[];
  materialId: number | null;
  setMaterialId: (id: number | null) => void;
  busy: string | null;
  onNext: () => void;
}) {
  // 解析不再把整份字表攤出來——那是一大塊沒有人會逐條讀的東西。
  // 這裡只提「這篇有幾個你不會的字」，要看意思就去點文章裡的那個字。
  const unknown = feedback.glossary?.filter((g) => g.is_unknown) ?? [];

  return (
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

      {unknown.length > 0 && (
        <p className="muted hint">
          這篇有 {unknown.length} 個你還不熟的字或片語（
          {unknown.slice(0, 8).map((g) => g.text).join("、")}
          {unknown.length > 8 && " …"}
          ）。點文章裡的那個字就會出現釋義，來源是你自己匯入的字典，不是 AI 寫的。
        </p>
      )}

      {feedback.taught_words?.length > 0 && (
        <p className="muted">
          這篇教的新字：{feedback.taught_words.join("、")}
          ——都已排進複習，之後的文章不會再拿同一批。
        </p>
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
          <MaterialPicker
            materials={materials}
            value={materialId}
            onChange={setMaterialId}
            disabled={busy !== null}
          />
        )}
        <button className="primary" onClick={onNext} disabled={busy !== null}>
          下一題
        </button>
      </div>
    </section>
  );
}

/** 選擇題，閱讀測驗與文法練習共用。 */
function Choices({
  items,
  choices,
  setChoices,
  feedback,
}: {
  items: {
    question: string;
    options: string[];
    answer_index: number;
    explanation: string | null;
    difficulty: string | null;
  }[];
  choices: (number | null)[];
  setChoices: (fn: (c: (number | null)[]) => (number | null)[]) => void;
  feedback: Feedback | null;
}) {
  return (
    <ol className="questions">
      {items.map((q, i) => (
        <li key={i}>
          <p className="prompt">
            {q.question}
            {q.difficulty && (
              <span className={`tag difficulty ${q.difficulty}`}>
                {DIFFICULTY_LABELS[q.difficulty] ?? q.difficulty}
              </span>
            )}
          </p>
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

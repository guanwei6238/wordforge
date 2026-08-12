import type React from "react";
import { useCallback, useEffect, useMemo, useState } from "react";
import {
  BLANK_PATTERN,
  currentLanguages,
  deleteExercise,
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
  type GrammarView,
  isGrammarKnown,
  listExercises,
  listGrammar,
  listMaterials,
  loadExercise,
  type Material,
  onDataReset,
  practiceStatus,
  type PracticeStatus,
  type ProfileLanguages,
  updateStudySettings,
} from "../api";
import LlmSetup from "../components/LlmSetup";
import SpeakButton from "../components/SpeakButton";

/** 一步調幾 px。太細要按很多次，太粗會跳過剛好的那一級。 */
const FONT_STEP = 1;
const FONT_MIN = 12;
const FONT_MAX = 32;

/** 練習紀錄一頁幾筆。一頁塞太多就等於沒有分頁。 */
const HISTORY_PAGE = 10;

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
  const [fontSize, setFontSize] = useState(16);
  // 練習紀錄。做過的題目可以整份叫回來重做
  const [history, setHistory] = useState<ExerciseSummary[]>([]);
  const [historyTotal, setHistoryTotal] = useState(0);
  const [historyPage, setHistoryPage] = useState(0);
  const [showHistory, setShowHistory] = useState(false);
  // 解析階段點到的字，顯示釋義用
  const [lookup, setLookup] = useState<string | null>(null);
  // 文法題要練哪一個點。空字串＝從今天到期的弱點裡挑（隨機）
  const [grammarPoints, setGrammarPoints] = useState<GrammarView[]>([]);
  const [grammarFocus, setGrammarFocus] = useState<string>("");

  const refresh = useCallback(async () => {
    try {
      setStatus(await practiceStatus());
    } catch (e) {
      setError(errorMessage(e));
    }
  }, []);

  const refreshHistory = useCallback(async (page: number) => {
    try {
      const got = await listExercises(HISTORY_PAGE, page * HISTORY_PAGE);
      // 刪到某一頁只剩空的時候要退回上一頁，不然畫面是一片空白
      if (got.items.length === 0 && page > 0) {
        setHistoryPage(page - 1);
        return;
      }
      setHistory(got.items);
      setHistoryTotal(got.total);
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
    void getStudySettings()
      .then((s) => setFontSize(s.reading_font_size))
      .catch(() => {});
    void listGrammar()
      .then(setGrammarPoints)
      .catch(() => {});
  }, [refresh]);

  useEffect(() => {
    void refreshHistory(historyPage);
  }, [refreshHistory, historyPage]);

  // 這一頁切走不會卸載（出一題要幾十秒，回來題目不能消失），
  // 代價是它看不到別的地方做了什麼。在設定頁按下重置之後，
  // 這裡還會顯示著已經不存在的題目與舊的詞彙量。
  useEffect(
    () =>
      onDataReset(() => {
        present(null);
        setShowHistory(false);
        setHistoryPage(0);
        void refresh();
        void refreshHistory(0);
        void getStudySettings()
          .then((s) => setFontSize(s.reading_font_size))
          .catch(() => {});
      }),
    [refresh, refreshHistory],
  );

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
        : ex?.body.kind === "choices" || ex?.body.kind === "cloze"
          ? ex.body.items.map(() => null)
          : [],
    );
  }

  async function start() {
    // 先清空。留著上一題的話，等模型的那幾十秒裡畫面顯示的是
    // 已經作廢的內容，而且捲到一半送出還會送到舊的 exercise_id。
    present(null);
    // 紀錄與題目擇一顯示：兩個都攤在同一頁時，出了新題目卻還看得到
    // 一整排舊的，很難分辨現在在做哪一份
    setShowHistory(false);
    setBusy("generating");
    setError(null);
    try {
      present(await generateExercise(kind, materialId, grammarFocus || null));
      await refreshHistory(historyPage);
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

  async function remove(exerciseId: number) {
    setError(null);
    try {
      await deleteExercise(exerciseId);
      // 刪掉的剛好是正在做的那份時，畫面上那份已經沒有對應的紀錄了，
      // 送出會找不到 exercise_id，所以一起收掉
      if (exercise?.exercise_id === exerciseId) {
        present(null);
      }
      await refreshHistory(historyPage);
    } catch (e) {
      setError(errorMessage(e));
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
      await refreshHistory(historyPage);
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
  const cloze = exercise?.body.kind === "cloze" ? exercise.body : null;
  // 有文章的題型才用寬版；而且紀錄攤開時要收回窄版，
  // 不然一張清單被拉到 1300px 寬會很難讀
  const wide = (reading || cloze) && !showHistory;

  // 沒作答的題數。全部答完才讓送出——漏掉一題送出去，
  // 批改會把它算成答錯，而那不是他的本意。
  const unanswered =
    exercise?.body.kind === "translation"
      ? answers.filter((a) => !a.trim()).length
      : choices.filter((c) => c == null).length;

  return (
    <div
      className={wide ? "practice wide" : "practice"}
      // 字級是一個設定，作用在整份練習上：文章、翻譯、題目、選項。
      // 只調文章的話，右邊的題目還是原本那麼小，眼睛在兩欄之間
      // 跳來跳去反而更累。
      style={{ "--exercise-font": `${fontSize}px` } as React.CSSProperties}
    >
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
            {/* 文法題才有意義：要練哪一個點。
                「隨機」是讓 FSRS 排程決定，「指定」是使用者自己挑。 */}
            {(kind === "grammar" ||
              (kind === "auto" && status.recommended === "grammar")) &&
              grammarPoints.length > 0 && (
                <label>
                  練哪個文法
                  <select
                    value={grammarFocus}
                    onChange={(e) => setGrammarFocus(e.target.value)}
                    disabled={busy !== null}
                  >
                    <option value="">隨機（從該複習的挑）</option>
                    {grammarPoints
                      .filter((g) => g.state != null)
                      .map((g) => (
                        <option key={g.point} value={g.point}>
                          {g.name}
                          {isGrammarKnown(g) ? "（已學會）" : ""}
                        </option>
                      ))}
                  </select>
                </label>
              )}
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
              {showHistory ? "回到題目" : `練習紀錄${historyTotal > 0 ? `（${historyTotal}）` : ""}`}
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

      {error && <p className="error">{error}</p>}

      {/* 紀錄與題目擇一顯示。兩個都攤在同一頁時，看不出現在在做哪一份 */}
      {showHistory ? (
        <History
          items={history}
          total={historyTotal}
          page={historyPage}
          labels={labels}
          disabled={busy !== null}
          currentId={exercise?.exercise_id ?? null}
          onPage={setHistoryPage}
          onRedo={redo}
          onDelete={remove}
        />
      ) : (
        <>
          {/* 閱讀測驗：文章在左、題目在右；批改完再插入一欄全文翻譯。
              翻譯排在原文旁邊才對照得起來，擺在最下面等於要一直上下捲。 */}
          {exercise && reading && (
            <div className={feedback && reading.translation ? "reading-layout three" : "reading-layout"}>
              <section className="panel exercise passage-pane">
                <PassageHeader
                  title={reading.title}
                  fontSize={fontSize}
                  onFontSize={changeFontSize}
                />
                <p className="muted hint">
                  {feedback
                    ? "點文章裡的任何一個字可以查它的意思。"
                    : "點任何一個字可以標記「我不會」，送出後會排進複習。"}
                  {exercise.coverage != null &&
                    `　這篇有 ${Math.round(exercise.coverage * 100)}% 的字你已經學過。`}
                </p>

                <p className="passage">
                  {reading.passage.split(/(\s+)/).map((chunk, i) => {
                    if (!chunk.trim()) return chunk;
                    const word = normalizeWord(chunk);
                    const isMarked = marked.some((w) => w.toLowerCase() === word);
                    return (
                      <span
                        key={i}
                        className={[
                          "token",
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
              </section>

              {/* 全文翻譯獨立一欄，跟原文並排。只在批改之後出現——
                  作答前給等於直接送答案。 */}
              {feedback && reading.translation && (
                <section className="panel translation-pane">
                  <h2>全文翻譯</h2>
                  <p className="passage">
                    {reading.translation}
                  </p>
                </section>
              )}

              <div className="answer-pane">
                {/* 點到的字的釋義自成一塊，而且釘在最上面：
                    夾在題目中間的話，看完要往回捲才找得到原文與翻譯。 */}
                {feedback && lookup && (
                  <WordNotes
                    term={lookup}
                    glossary={feedback.glossary ?? []}
                    onClose={() => setLookup(null)}
                  />
                )}

                <section className="panel exercise">
                  <Choices
                    items={reading.questions}
                    choices={choices}
                    setChoices={setChoices}
                    feedback={feedback}
                  />
                  {!feedback && (
                    <SubmitRow
                      unanswered={unanswered}
                      busy={busy}
                      onSubmit={submit}
                      what="題"
                    />
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

          {/* 克漏字：短文在左、每一格的選項在右，批改完中間插入翻譯。
              跟閱讀測驗一樣的版面——同樣是「對照原文看」的需求。 */}
          {exercise && cloze && (
            <div
              className={
                feedback && cloze.translation ? "reading-layout three" : "reading-layout"
              }
            >
              <section className="panel exercise passage-pane">
                <PassageHeader
                  title={cloze.title}
                  fontSize={fontSize}
                  onFontSize={changeFontSize}
                />
                <p className="muted hint">
                  文章裡的每一個空格對應右邊同號的一題。這些字你都學過，
                  考的是在句子裡想不想得起來。
                </p>
                <ClozePassage
                  passage={cloze.passage}
                  items={cloze.items}
                  choices={choices}
                  feedback={feedback}
                  lookup={lookup}
                  onLookup={setLookup}
                />
                {feedback && (
                  <p className="muted hint">點文章裡的任何一個字可以查它的意思。</p>
                )}
              </section>

              {feedback && cloze.translation && (
                <section className="panel translation-pane">
                  <h2>全文翻譯</h2>
                  <p className="passage">{cloze.translation}</p>
                </section>
              )}

              <div className="answer-pane">
                {feedback && lookup && (
                  <WordNotes
                    term={lookup}
                    glossary={feedback.glossary ?? []}
                    onClose={() => setLookup(null)}
                  />
                )}

                <section className="panel exercise">
                  <Choices
                    items={cloze.items}
                    choices={choices}
                    setChoices={setChoices}
                    feedback={feedback}
                    numbered
                    compact
                  />
                  {!feedback && (
                    <SubmitRow
                      unanswered={unanswered}
                      busy={busy}
                      onSubmit={submit}
                      what="格"
                    />
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

          {exercise && !reading && !cloze && (
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
                <SubmitRow
                  unanswered={unanswered}
                  busy={busy}
                  onSubmit={submit}
                  what="題"
                />
              )}
            </section>
          )}

          {feedback && !reading && !cloze && (
            <FeedbackPanel
              feedback={feedback}
              materials={materials}
              materialId={materialId}
              setMaterialId={setMaterialId}
              busy={busy}
              onNext={start}
            />
          )}
        </>
      )}
    </div>
  );
}

/** 跟後端 `wordforge_core::text::normalize` 對得上的比對鍵。 */
function normalizeWord(raw: string): string {
  return raw.replace(/[^\p{L}\p{N}'-]/gu, "").toLowerCase();
}

/** 標題與字級調整。閱讀與克漏字共用。 */
function PassageHeader({
  title,
  fontSize,
  onFontSize,
}: {
  title: string;
  fontSize: number;
  onFontSize: (next: number) => void;
}) {
  return (
    <div className="row title-row">
      <h2>{title}</h2>
      <span className="font-size">
        <button
          onClick={() => onFontSize(fontSize - FONT_STEP)}
          disabled={fontSize <= FONT_MIN}
          title="縮小文章字級"
        >
          A−
        </button>
        <span className="muted">{fontSize}px</span>
        <button
          onClick={() => onFontSize(fontSize + FONT_STEP)}
          disabled={fontSize >= FONT_MAX}
          title="放大文章字級"
        >
          A+
        </button>
      </span>
    </div>
  );
}

/**
 * 送出按鈕。全部答完才能按。
 *
 * 漏掉一題送出去，批改會把它算成答錯，而那不是他的本意——
 * 而且錯誤會被記進文法弱點，之後一直出那個文法點的題目。
 */
function SubmitRow({
  unanswered,
  busy,
  onSubmit,
  what,
}: {
  unanswered: number;
  busy: string | null;
  onSubmit: () => void;
  what: string;
}) {
  return (
    <div className="row submit-row">
      <button
        className="primary"
        onClick={onSubmit}
        disabled={busy !== null || unanswered > 0}
      >
        {busy === "grading" ? "批改中…" : "送出"}
      </button>
      {unanswered > 0 && (
        <span className="muted">
          還有 {unanswered} {what}沒作答
        </span>
      )}
    </div>
  );
}

/** 克漏字的短文，空格處顯示編號或已填的字。 */
function ClozePassage({
  passage,
  items,
  choices,
  feedback,
  lookup,
  onLookup,
}: {
  passage: string;
  items: { options: string[]; answer_index: number }[];
  choices: (number | null)[];
  feedback: Feedback | null;
  /** 解析時點開的那個字。作答前是 null——那時候給翻譯等於送答案 */
  lookup: string | null;
  onLookup: (term: string | null) => void;
}) {
  // 依 {{n}} 切開。用 split 保留分隔符，一次走完不必自己算位置。
  const parts = useMemo(() => passage.split(new RegExp(BLANK_PATTERN.source, "g")), [passage]);

  return (
    <p className="passage">
      {parts.map((part, i) => {
        // split 帶捕獲群組時，奇數索引是空格編號
        // 偶數段是文章本文。批改之後每個字都能點開查意思，
        // 作答前不行——那時候給翻譯等於直接送答案。
        if (i % 2 === 0) {
          if (!feedback) return <span key={i}>{part}</span>;
          return (
            <span key={i}>
              {part.split(/(\s+)/).map((chunk, j) => {
                if (!chunk.trim()) return chunk;
                const word = normalizeWord(chunk);
                return (
                  <span
                    key={j}
                    className={lookup === word ? "token looking" : "token"}
                    onClick={() => onLookup(lookup === word ? null : word)}
                  >
                    {chunk}
                  </span>
                );
              })}
            </span>
          );
        }

        const n = Number(part);
        const item = items[n - 1];
        const picked = choices[n - 1];
        const correct = item != null && picked === item.answer_index;
        const filled = item != null && picked != null ? item.options[picked] : null;

        return (
          <span
            key={i}
            className={[
              "blank",
              filled ? "filled" : "",
              feedback ? (correct ? "right" : "wrong") : "",
            ]
              .filter(Boolean)
              .join(" ")}
          >
            <sup>{n}</sup>
            {/* 批改後直接顯示正確答案，才對得起旁邊的解說 */}
            {feedback && item ? item.options[item.answer_index] : (filled ?? "＿＿＿")}
          </span>
        );
      })}
    </p>
  );
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
  total,
  page,
  labels,
  disabled,
  currentId,
  onPage,
  onRedo,
  onDelete,
}: {
  items: ExerciseSummary[];
  total: number;
  page: number;
  labels: Record<ExerciseKind, string>;
  disabled: boolean;
  currentId: number | null;
  onPage: (page: number) => void;
  onRedo: (id: number) => void;
  onDelete: (id: number) => void;
}) {
  // 刪除要二次確認，但不用彈窗——按一下變成「確定刪除」，
  // 按別的地方或再點一次別份就恢復
  const [confirming, setConfirming] = useState<number | null>(null);
  const pages = Math.max(1, Math.ceil(total / HISTORY_PAGE));

  if (total === 0) {
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
          <li key={it.exercise_id} className={it.exercise_id === currentId ? "current" : ""}>
            <div>
              <span className="tag">{labels[it.kind] ?? it.kind}</span>
              <span className="history-title">{it.title}</span>
            </div>
            <div className="muted history-meta">
              {formatWhen(it.created_at)}
              {it.score != null ? `　·　${Math.round(it.score)} 分` : "　·　沒作答"}
              {it.coverage != null && `　·　覆蓋率 ${Math.round(it.coverage * 100)}%`}
            </div>
            <div className="history-actions">
              <button onClick={() => onRedo(it.exercise_id)} disabled={disabled}>
                再做一次
              </button>
              {confirming === it.exercise_id ? (
                <>
                  <button
                    className="destructive"
                    onClick={() => {
                      setConfirming(null);
                      onDelete(it.exercise_id);
                    }}
                    disabled={disabled}
                  >
                    確定刪除
                  </button>
                  <button onClick={() => setConfirming(null)}>取消</button>
                </>
              ) : (
                <button onClick={() => setConfirming(it.exercise_id)} disabled={disabled}>
                  刪除
                </button>
              )}
            </div>
          </li>
        ))}
      </ul>

      {pages > 1 && (
        <div className="row pager">
          <button onClick={() => onPage(page - 1)} disabled={page === 0 || disabled}>
            上一頁
          </button>
          <span className="muted">
            第 {page + 1} / {pages} 頁　·　共 {total} 份
          </span>
          <button onClick={() => onPage(page + 1)} disabled={page + 1 >= pages || disabled}>
            下一頁
          </button>
        </div>
      )}

      <p className="muted hint">
        重做的是同一份題目，不會再花一次出題的額度；送出後一樣由模型批改，
        舊的那次批改也留著。刪掉的話那份題目與所有作答一起消失，沒有復原。
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

/** 批改結果。有文章的題型擺在右欄，其他擺在下面。 */
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

/** 選擇題，閱讀測驗、克漏字與文法練習共用。 */
function Choices({
  items,
  choices,
  setChoices,
  feedback,
  numbered = false,
  compact = false,
}: {
  items: {
    question: string;
    options: string[];
    option_notes: string[];
    answer_index: number;
    explanation: string | null;
    difficulty: string | null;
  }[];
  choices: (number | null)[];
  setChoices: (fn: (c: (number | null)[]) => (number | null)[]) => void;
  feedback: Feedback | null;
  /** 克漏字要標「第 N 格」才對得回文章裡的空格 */
  numbered?: boolean;
  /**
   * 選項橫排。
   *
   * 克漏字的選項就是幾個單字或片語，一個一列的話一題佔掉四行，
   * 八格要捲很久。閱讀與文法的選項是整句話，橫排會擠成一團，
   * 所以這件事不能一體適用，要由呼叫端決定。
   */
  compact?: boolean;
}) {
  return (
    <ol className={numbered ? "questions blanks" : "questions"}>
      {items.map((q, i) => (
        <li key={i}>
          <p className="prompt">
            {numbered && <span className="blank-no">第 {i + 1} 格</span>}
            {q.question}
            {q.difficulty && (
              <span className={`tag difficulty ${q.difficulty}`}>
                {DIFFICULTY_LABELS[q.difficulty] ?? q.difficulty}
              </span>
            )}
          </p>
          <div className={compact ? "options compact" : "options"}>
            {q.options.map((option, j) => (
              <label
                key={j}
                className={[
                  "option",
                  feedback && j === q.answer_index ? "right" : "",
                  feedback && choices[i] === j && j !== q.answer_index ? "picked-wrong" : "",
                ]
                  .filter(Boolean)
                  .join(" ")}
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
          {feedback && (
            <Verdict
              item={q}
              picked={choices[i] ?? null}
              comment={feedback.items[i]?.comment ?? null}
            />
          )}
        </li>
      ))}
    </ol>
  );
}

/**
 * 一題的講評。
 *
 * 重點是**先講你選的那一個**。只講「正確答案為什麼對」對答錯的人沒有
 * 用——他要知道的是自己那條路錯在哪，不然下次還是會被同一個選項騙到。
 *
 * 三個來源，由具體到一般：
 *
 * 1. `option_notes[你選的]`——出題時就替每個選項各備一句。選擇題在
 *    本地判分，模型沒看過你的作答，所以這是唯一「認得出你選了什麼」
 *    而且不必多打一次模型的來源。
 * 2. `comment`——批改時模型寫的。只有閱讀與翻譯有（那兩種會真的送去
 *    批改），而且它看得到你的作答。
 * 3. `explanation`——整題在考什麼，跟你選什麼無關。
 */
function Verdict({
  item,
  picked,
  comment,
}: {
  item: { options: string[]; option_notes: string[]; answer_index: number; explanation: string | null };
  picked: number | null;
  comment: string | null;
}) {
  const note = (index: number | null) =>
    index == null ? null : (item.option_notes[index]?.trim() || null);

  const correct = picked === item.answer_index;
  const pickedNote = correct ? null : note(picked);
  const answerNote = note(item.answer_index);

  // 模型的講評跟出題時的說明有時會撞在一起，一模一樣就只留一份
  const extra =
    comment && comment.trim() && comment.trim() !== item.explanation?.trim()
      ? comment.trim()
      : null;

  return (
    <div className="verdict">
      {!correct && (
        <p className="error">
          {picked == null ? (
            <>沒有作答</>
          ) : (
            <>
              你選了「{item.options[picked]}」
              {pickedNote && <span className="muted">：{pickedNote}</span>}
            </>
          )}
        </p>
      )}
      {answerNote && (
        <p className={correct ? "ok" : "muted"}>
          {correct ? "✓ " : ""}
          正解「{item.options[item.answer_index]}」：{answerNote}
        </p>
      )}
      {extra && <p className="muted">{extra}</p>}
      {item.explanation && <p className="muted">{item.explanation}</p>}
    </div>
  );
}

import type React from "react";
import { useCallback, useEffect, useState } from "react";
import {
  addWord,
  currentLanguages,
  deleteExercise,
  errorMessage,
  exerciseLabels,
  type ExerciseKind,
  type ExerciseSummary,
  type ExerciseView,
  type Feedback,
  generateExercise,
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
} from "../../api";
import { Choices } from "./choices";
import LlmSetup from "../../components/LlmSetup";
import SpeakButton from "../../components/SpeakButton";
import { FeedbackPanel, SubmitRow } from "./feedback";
import { History, HISTORY_PAGE, MaterialPicker } from "./history";
import {
  ClozePassage,
  FONT_MAX,
  FONT_MIN,
  normalizeWord,
  PassageHeader,
  WordNotes,
} from "./passage";



/**
 * 文法點依「學到哪」分組，給「練哪個文法」的選單用。
 *
 * 這個函式存在的理由是它曾經不存在：選單原本寫死
 * `.filter((g) => g.state != null)`，也就是**練過的才選得到**。
 * 使用者自己新增或匯入的點 `state` 一律是 `null`，於是永遠不會出現在
 * 選單裡——想練它就得先在別的地方錯一次，而文法題本來就只會考
 * 「該複習的點」，那一次永遠不會發生。「匯入什麼就能學什麼」
 * 對文法等於沒有兌現。
 *
 * 分組而不是攤平：全部列出來會是四十幾個點的一長串，
 * 學到哪一段看不出來。
 */
function grammarGroups(points: GrammarView[]): { label: string; items: GrammarView[] }[] {
  return [
    { label: "學習中", items: points.filter((g) => g.state != null && !isGrammarKnown(g)) },
    { label: "還沒練過", items: points.filter((g) => g.state == null) },
    { label: "已學會", items: points.filter(isGrammarKnown) },
  ].filter((group) => group.items.length > 0);
}

/**
 * AI 練習頁。
 *
 * 一整條迴圈都在這裡：依程度出題 → 作答 → 批改 → 不會的字自動排進複習。
 *
 * 閱讀測驗與克漏字分兩個階段，點文章的意思也跟著換：
 *
 * - **作答前**：點任何一個字＝標記「我不會」，送出時一起排進複習。
 * - **解析時**：點任何一個字＝查它的釋義。這時候才給翻譯，
 *   作答前給等於直接送答案。解析時才發現不會的字，
 *   在釋義視窗裡按「＋複習」補加——那時候 `marked_unknown` 已經送出去了。
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
  // 覆盤時另外補加進牌組的字。跟 marked 分開：那個是送出時一起帶的，
  // 這個是批改完才按的，當下就直接寫進牌組了
  const [addedLater, setAddedLater] = useState<string[]>([]);
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
    setAddedLater([]);
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

  /** 覆盤時補加一個字進牌組。
   *
   * 樂觀更新：先標成「已加入」再送，失敗的話收回並顯示錯誤。
   * 這個按鈕按下去到卡片出現在複習佇列之間沒有別的回饋，
   * 讓它等一趟 round-trip 會像沒反應。 */
  async function addLater(word: string) {
    const clean = word.trim();
    if (!clean || addedLater.some((w) => w.toLowerCase() === clean.toLowerCase())) return;
    setAddedLater((w) => [...w, clean]);
    try {
      await addWord(clean, langs.target);
      await refresh();
    } catch (e) {
      setAddedLater((w) => w.filter((x) => x !== clean));
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
                    {grammarGroups(grammarPoints).map((group) => (
                      <optgroup key={group.label} label={group.label}>
                        {group.items.map((g) => (
                          <option key={g.point} value={g.point}>
                            {g.name}
                            {g.level ? `（${g.level}）` : ""}
                          </option>
                        ))}
                      </optgroup>
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
                    onAdd={addLater}
                    added={addedLater}
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
                  {!feedback && "　點空格以外的任何一個字可以標記「我不會」，送出後會排進複習。"}
                </p>
                <ClozePassage
                  passage={cloze.passage}
                  items={cloze.items}
                  choices={choices}
                  feedback={feedback}
                  lookup={lookup}
                  onLookup={setLookup}
                  marked={marked}
                  onToggleMark={toggleMarked}
                />
                {marked.length > 0 && !feedback && (
                  <p className="muted">標記為不會的字：{marked.join("、")}</p>
                )}
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
                    onAdd={addLater}
                    added={addedLater}
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
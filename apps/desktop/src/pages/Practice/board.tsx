/**
 * 練習題目的完整畫面，練習頁與練習紀錄共用。
 */
import type React from "react";
import {
  type ExerciseView,
  type Feedback,
  languageName,
  type Material,
  type ProfileLanguages,
} from "../../api";
import Reference from "../../components/Reference";
import SpeakButton from "../../components/SpeakButton";
import { Choices } from "./choices";
import { FeedbackPanel, SubmitRow } from "./feedback";
import { ClozePassage, normalizeWord, PassageHeader, WordNotes } from "./passage";

/**
 * 一份練習的畫面：題目、作答、批改後的解析。
 *
 * 練習頁與練習紀錄**共用這一份**。紀錄要顯示的正是「當時做完、批改完
 * 長什麼樣」，自己再寫一套逐題摘要只會有兩份互相漂移的版面——而且
 * 那一套永遠比較簡陋（沒有文章、沒有選項、沒有生字表）。
 *
 * 「唯讀」不是一個旗標：畫面上每個互動本來就綁在 `feedback` 上
 * （批改完就不能改答案了），紀錄頁只是額外不傳 `onNext`，
 * 於是「下一題」與教材選單自然不出現。
 */
export default function ExerciseBoard({
  exercise,
  langs,
  fontSize,
  onFontSize,
  answers,
  choices,
  marked,
  lookup,
  feedback,
  setLookup,
  addedLater,
  onAddLater,
  setAnswers,
  setChoices,
  onToggleMark,
  onSubmit,
  busy,
  unanswered,
  materials,
  materialId,
  setMaterialId,
  onNext,
}: {
  exercise: ExerciseView;
  langs: ProfileLanguages;
  fontSize: number;
  onFontSize: (next: number) => void;
  answers: string[];
  choices: (number | null)[];
  marked: string[];
  lookup: string | null;
  feedback: Feedback | null;
  setLookup: React.Dispatch<React.SetStateAction<string | null>>;
  addedLater: string[];
  onAddLater: (word: string) => void;
  setAnswers: React.Dispatch<React.SetStateAction<string[]>>;
  setChoices: React.Dispatch<React.SetStateAction<(number | null)[]>>;
  onToggleMark: (word: string) => void;
  onSubmit: () => void;
  busy: "generating" | "grading" | "loading" | null;
  unanswered: number;
  materials: Material[];
  materialId: number | null;
  setMaterialId: (id: number | null) => void;
  /** 不傳就沒有「下一題」——看歷史紀錄時沒有下一題可出 */
  onNext?: () => void;
}) {
  const reading = exercise.body.kind === "reading" ? exercise.body : null;
  const cloze = exercise.body.kind === "cloze" ? exercise.body : null;

  return (
    <>
          {/* 閱讀測驗：文章在左、題目在右；批改完再插入一欄全文翻譯。
              翻譯排在原文旁邊才對照得起來，擺在最下面等於要一直上下捲。 */}
          {exercise && reading && (
            <div className={feedback && reading.translation ? "reading-layout three" : "reading-layout"}>
              <section className="panel exercise passage-pane">
                <PassageHeader
                  title={reading.title}
                  fontSize={fontSize}
                  onFontSize={onFontSize}
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
                            : onToggleMark(chunk)
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
                    onAdd={onAddLater}
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
                      onSubmit={onSubmit}
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
                    onNext={onNext}
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
                  onFontSize={onFontSize}
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
                  onToggleMark={onToggleMark}
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
                    onAdd={onAddLater}
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
                      onSubmit={onSubmit}
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
                    onNext={onNext}
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
                          <Reference
                            reference={feedback.items[i].reference}
                            formal={feedback.items[i].reference_formal}
                          />
                          {feedback.items[i].comment && <span>　{feedback.items[i].comment}</span>}
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
                  onSubmit={onSubmit}
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
              onNext={onNext}
            />
          )}
    </>
  );
}

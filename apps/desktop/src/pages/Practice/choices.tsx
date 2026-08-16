/**
 * 選擇題的畫面：題目、選項、答完之後的對錯與逐選項解說。
 *
 * 閱讀測驗、克漏字與文法練習共用同一組元件——它們送到前端的形狀本來就一樣。
 */
import {
  DIFFICULTY_LABELS,
  type Feedback,
} from "../../api";

/** 選擇題，閱讀測驗、克漏字與文法練習共用。 */
export function Choices({
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
export function Verdict({
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

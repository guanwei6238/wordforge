/** 批改結果與送出列。有文章的題型擺在右欄，其他擺在下面。 */
import {
  type Feedback,
  type Material,
} from "../../api";
import { MaterialPicker } from "./history";

/**
 * 送出按鈕。全部答完才能按。
 *
 * 漏掉一題送出去，批改會把它算成答錯，而那不是他的本意——
 * 而且錯誤會被記進文法弱點，之後一直出那個文法點的題目。
 */
export function SubmitRow({
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

/** 批改結果。有文章的題型擺在右欄，其他擺在下面。 */
export function FeedbackPanel({
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

/**
 * 逐處修正：你寫的哪一段、該改成什麼、為什麼。
 *
 * 這是整份批改裡最有教學價值的一塊。`comment` 只是一句摘要——
 * 「缺少正在進行式，也沒有使用本題要練的 address」指出了問題，
 * 但沒有說該怎麼寫；要看到 `dealing with` → `is addressing`
 * 得靠這一份。
 *
 * 練習頁與複習頁共用，因為它們批改回來的是同一種東西。複習那邊
 * 曾經把這份資料拿去記文法點之後就丟掉，畫面只剩那一句摘要——
 * 使用者的原話是「llm 也沒說我的寫法要怎麼改才是正確的」。
 */
import type { Correction } from "../api";

export default function Corrections({ items }: { items: Correction[] }) {
  if (items.length === 0) return null;
  return (
    <ul className="corrections">
      {items.map((c, i) => (
        <li key={i}>
          <span className="wrong">{c.original}</span>
          {" → "}
          <span className="right">{c.corrected}</span>
          {c.grammar_point && <span className="tag">{c.grammar_point}</span>}
          {c.explanation && <p className="muted">{c.explanation}</p>}
        </li>
      ))}
    </ul>
  );
}

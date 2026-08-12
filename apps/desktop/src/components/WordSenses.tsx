import { tagLabel, type WordDetail } from "../api";

/**
 * 一個詞條的完整內容：發音、所有義項、例句、變化形。
 *
 * 字典頁與複習頁共用。**不包含**標題與「加入牌組」按鈕——
 * 兩邊的抬頭需求不一樣（字典頁要大標題與加入按鈕，複習頁那個字
 * 已經是卡片的正面了），硬塞進來只會讓兩邊都得傳一堆開關。
 *
 * `showTags` 是唯一的例外：字典頁的抬頭已經列過考試標籤了，
 * 兩邊都畫會變成同一排標籤出現兩次。
 */
export default function WordSenses({
  detail,
  showTags = true,
}: {
  detail: WordDetail;
  showTags?: boolean;
}) {
  return (
    <div className="word-senses">
      {detail.pronunciations.length > 0 && (
        <p className="prons">
          {detail.pronunciations.map((p, i) => (
            <span key={i} className="pron">
              {p.accent && <span className="tag">{p.accent}</span>}
              {p.ipa}
              {p.is_synthetic && <span className="tag">合成音</span>}
            </span>
          ))}
        </p>
      )}

      <ol className="senses">
        {detail.senses.map((s, i) => (
          <li key={i}>
            <p className="gloss">
              {s.pos && <span className="tag pos">{s.pos}</span>}
              {s.gloss}
            </p>
            {s.translation && <p className="translation">{s.translation}</p>}
            {(s.register || s.domain) && (
              <p className="labels">
                {s.register && <span className="tag">{s.register}</span>}
                {s.domain && <span className="tag">{s.domain}</span>}
              </p>
            )}
            {s.examples.map((ex, j) => (
              <p key={j} className="example">
                {ex.text}
                {ex.translation && <span className="muted"> — {ex.translation}</span>}
              </p>
            ))}
            {/* CC BY-SA 的標示義務，不能省略 */}
            {s.attribution && <p className="attribution">— {s.attribution}</p>}
          </li>
        ))}
      </ol>

      {detail.forms.length > 0 && (
        <p className="forms">
          <span className="muted">變化形：</span>
          {detail.forms.map(([form, tag], i) => (
            <span key={i} className="tag" title={tag}>
              {form}
            </span>
          ))}
        </p>
      )}

      {showTags && detail.tags.length > 0 && (
        <p className="forms">
          <span className="muted">收錄於：</span>
          {detail.tags.map((t) => (
            <span key={t} className="tag exam" title={t}>
              {tagLabel(t)}
            </span>
          ))}
        </p>
      )}
    </div>
  );
}

import { useEffect, useState } from "react";

import {
  SENTENCE_ORIGIN_LABELS,
  tagLabel,
  type WordDetail,
  type WordSentence,
  wordSentences,
} from "../api";

/** 右欄一次放幾句。 */
const SENTENCES_PER_PAGE = 5;

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
      <div className="senses-main">
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

      {/* 做過的句子擺右邊，不接在釋義後面：義項多的字有十幾條，
          「我用過這個字」會被推到捲不到的地方，而那正是最該看的一塊 */}
      <MySentences lemmaId={detail.lemma_id} />
    </div>
  );
}

/**
 * 「這個字我在哪句話裡用過」。
 *
 * 上面那些例句是字典收錄的——別人寫的句子。這一段是**自己做過的**：
 * 翻譯題裡寫過的、閱讀文章裡讀到的那一行。同一個字，後者記得住的
 * 機會高得多，因為那是有情境的一次真實使用。
 *
 * 分頁而不是一次列完：常練的字會累積到十幾句，全部攤開會把整個
 * 右欄拉得比釋義還長。
 *
 * 沒有連結時整欄不顯示（新使用者、或這個字只出現在字典裡），
 * 不留一個「你做過的句子：（沒有）」的空殼。
 */
function MySentences({ lemmaId }: { lemmaId: number }) {
  const [page, setPage] = useState(0);
  const [sentences, setSentences] = useState<WordSentence[]>([]);
  const [total, setTotal] = useState(0);

  // 換一個字要從第一頁看起，不然會停在上一個字的第三頁（多半是空的）
  useEffect(() => {
    setPage(0);
  }, [lemmaId]);

  useEffect(() => {
    let cancelled = false;
    void wordSentences(lemmaId, SENTENCES_PER_PAGE, page * SENTENCES_PER_PAGE)
      .then((got) => {
        if (cancelled) return;
        setSentences(got.items);
        setTotal(got.total);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [lemmaId, page]);

  if (total === 0) return null;
  const pages = Math.max(1, Math.ceil(total / SENTENCES_PER_PAGE));

  return (
    <aside className="my-sentences">
      <p className="muted">你做過的句子（{total}）</p>
      <ul>
        {sentences.map((s) => (
          <li key={s.id}>
            <p className="mine-text">{s.text}</p>
            {s.translation && <p className="muted">{s.translation}</p>}
            <span className="tag">{SENTENCE_ORIGIN_LABELS[s.origin] ?? s.origin}</span>
            {/* 錯過幾次是複習時最有用的訊號：這句我卡過三次 */}
            {s.misses > 0 && <span className="tag miss">錯過 {s.misses} 次</span>}
          </li>
        ))}
      </ul>
      {pages > 1 && (
        <div className="row sentence-pager">
          <button onClick={() => setPage(page - 1)} disabled={page === 0}>
            ←
          </button>
          <span className="muted">
            {page + 1} / {pages}
          </span>
          <button onClick={() => setPage(page + 1)} disabled={page + 1 >= pages}>
            →
          </button>
        </div>
      )}
    </aside>
  );
}

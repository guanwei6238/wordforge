/**
 * 文章型題目的畫面：標題列、克漏字的短文、點到某個字時的釋義。
 *
 * 閱讀與克漏字共用這些——兩者的差別在考什麼（看不看得懂 vs 想不想得起來），
 * 不在怎麼呈現一篇文章。
 */
import { useMemo } from "react";
import {
  BLANK_PATTERN,
  type Feedback,
  type GlossaryNote,
} from "../../api";
import SpeakButton from "../../components/SpeakButton";

/** 跟後端 `wordforge_core::text::normalize` 對得上的比對鍵。 */
/** 一步調幾 px。太細要按很多次，太粗會跳過剛好的那一級。 */
export const FONT_STEP = 1;
export const FONT_MIN = 12;
export const FONT_MAX = 32;

/** 跟後端 `wordforge_core::text::normalize` 對得上的比對鍵。 */
export function normalizeWord(raw: string): string {
  return raw.replace(/[^\p{L}\p{N}'-]/gu, "").toLowerCase();
}

/** 標題與字級調整。閱讀與克漏字共用。 */
export function PassageHeader({
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

/** 克漏字的短文，空格處顯示編號或已填的字。 */
export function ClozePassage({
  passage,
  items,
  choices,
  feedback,
  lookup,
  onLookup,
  marked,
  onToggleMark,
}: {
  passage: string;
  items: { options: string[]; answer_index: number }[];
  choices: (number | null)[];
  feedback: Feedback | null;
  /** 解析時點開的那個字。作答前是 null——那時候給翻譯等於送答案 */
  lookup: string | null;
  onLookup: (term: string | null) => void;
  marked: string[];
  onToggleMark: (word: string) => void;
}) {
  // 依 {{n}} 切開。用 split 保留分隔符，一次走完不必自己算位置。
  const parts = useMemo(() => passage.split(new RegExp(BLANK_PATTERN.source, "g")), [passage]);

  return (
    <p className="passage">
      {parts.map((part, i) => {
        // split 帶捕獲群組時，奇數索引是空格編號
        // 偶數段是文章本文。跟閱讀測驗同一套規則：作答前點字是標記
        // 「我不會」，批改後才是查釋義——作答前給翻譯等於直接送答案。
        // 空格本身不在這裡，所以標記不會洩漏任何一題的答案。
        if (i % 2 === 0) {
          return (
            <span key={i}>
              {part.split(/(\s+)/).map((chunk, j) => {
                if (!chunk.trim()) return chunk;
                const word = normalizeWord(chunk);
                const isMarked = marked.some((w) => w.toLowerCase() === word);
                return (
                  <span
                    key={j}
                    className={[
                      "token",
                      isMarked ? "marked" : "",
                      feedback && lookup === word ? "looking" : "",
                    ]
                      .filter(Boolean)
                      .join(" ")}
                    onClick={() =>
                      feedback ? onLookup(lookup === word ? null : word) : onToggleMark(chunk)
                    }
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
export function WordNotes({
  term,
  glossary,
  onClose,
  onAdd,
  added,
}: {
  term: string;
  glossary: GlossaryNote[];
  onClose: () => void;
  /** 覆盤時才發現不會的字，這裡補加進牌組。作答前的標記走 marked_unknown */
  onAdd: (word: string) => void;
  added: string[];
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
          {hits.map((g, i) => {
            const isAdded = added.some((w) => w.toLowerCase() === g.term.toLowerCase());
            return (
              <div key={i} className="gloss-row">
                <dt>
                  {g.text}
                  {g.is_phrase && (
                    <span className="tag" title="片語：單看每個字查不出這個意思">
                      片語
                    </span>
                  )}
                  <SpeakButton text={g.text} />
                  <button
                    className="add-review"
                    disabled={isAdded}
                    onClick={() => onAdd(g.term)}
                    title={isAdded ? "已經在牌組裡了" : "把這個詞加進複習牌組"}
                  >
                    {isAdded ? "已加入" : "＋複習"}
                  </button>
                </dt>
                <dd>
                  {/* 翻譯與目標語言定義來自不同字典，兩個都給。
                      相同的話只顯示一次——ECDICT 兩邊都填了同一句。 */}
                  {g.translation && <span className="translation">{g.translation}</span>}
                  {g.gloss && g.gloss !== g.translation && (
                    <span className="gloss">{g.gloss}</span>
                  )}
                  {!g.translation && !g.gloss && <span className="muted">查無釋義</span>}
                </dd>
              </div>
            );
          })}
        </dl>
      )}
    </div>
  );
}

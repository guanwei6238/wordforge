import { useCallback, useEffect, useState } from "react";

import {
  errorMessage,
  grammarLevels,
  type LevelOption,
  type StudySettings,
} from "../api";

/**
 * 句子難度上限。
 *
 * ## 為什麼這一頁看起來這麼「空」
 *
 * 沒有匯入任何句型之前，分級那一格是空的，而且**應該**是空的：
 * 分級體系是資料，程式不預設任何一套（台灣課綱、CEFR、JLPT 都得成立）。
 * 這跟「設定頁的語言選單來自字典裡有什麼」是同一件事。
 *
 * ## 為什麼要露出「偵測得到幾個」
 *
 * 偵測不到的句型只能靠模型自報，難度上限對它不生效。兩個數字差很多的話
 * 使用者該知道這一級的保護其實很薄——不然他會以為系統擋得住，
 * 而題目照樣很難。
 */
export default function SentenceDifficulty({
  settings,
  save,
}: {
  settings: StudySettings;
  save: (next: StudySettings) => void;
}) {
  const [levels, setLevels] = useState<LevelOption[]>([]);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setLevels(await grammarLevels());
    } catch (e) {
      setError(errorMessage(e));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const current = levels.find((l) => l.ordinal === settings.pattern_ceiling);
  const detectable = levels
    .filter((l) => settings.pattern_ceiling == null || l.ordinal <= settings.pattern_ceiling)
    .reduce((n, l) => n + l.detectable, 0);

  return (
    <section className="panel">
      <h2>句子難度</h2>

      {levels.length === 0 ? (
        <p className="muted hint">
          還沒有句型可以分級。到<strong>文法</strong>頁匯入一份句型清單，
          或請 AI 草擬一批，這裡就會出現對應的程度選項。
          分級用的是清單自己帶的等級——匯入台灣課綱就是國小國中高中，
          匯入 CEFR 就是 A1 到 C2。
        </p>
      ) : (
        <>
          <label>
            我現在的句型程度
            <select
              value={settings.pattern_ceiling ?? ""}
              onChange={(e) =>
                save({
                  ...settings,
                  pattern_ceiling: e.target.value === "" ? null : Number(e.target.value),
                })
              }
            >
              <option value="">不限制</option>
              {levels.map((l) => (
                <option key={l.ordinal} value={l.ordinal}>
                  {l.level ?? `第 ${l.ordinal} 級`}
                </option>
              ))}
            </select>
          </label>
          <p className="muted hint">
            {current ? (
              <>
                出題只會用到「{current.level ?? `第 ${current.ordinal} 級`}」
                以下的句型。超過的會在出題之後被系統實際比對出來，退回去重寫。
              </>
            ) : (
              <>目前不限制句型難度。設一個程度之後，超過的句型會被退回去重寫。</>
            )}
          </p>
          <p className="muted hint">
            這個上限之內有 {detectable} 個句型<strong>偵測得到</strong>。
            偵測不到的句型只能靠 AI 自己遵守，系統擋不住——
            所以這個數字比句型總數重要。
          </p>
        </>
      )}

      <label>
        一句話最多幾個詞
        <input
          type="number"
          min={3}
          max={60}
          step={1}
          value={settings.sentence_max_words ?? ""}
          placeholder="不限"
          onChange={(e) =>
            save({
              ...settings,
              sentence_max_words: e.target.value === "" ? null : Number(e.target.value),
            })
          }
        />
      </label>
      <label>
        一句話最多幾個子句
        <input
          type="number"
          min={0}
          max={10}
          step={1}
          value={settings.sentence_max_clauses ?? ""}
          placeholder="不限"
          onChange={(e) =>
            save({
              ...settings,
              sentence_max_clauses: e.target.value === "" ? null : Number(e.target.value),
            })
          }
        />
      </label>
      <p className="muted hint">
        這兩個是<strong>粗但全面</strong>的那一層：句型偵測只抓得到清單裡有的東西，
        而沒收錄的難句正是最需要被擋下來的。子句填 0 就是「只給單句」。
        中日文沒有空格，「詞」會以字數計算，數字要另外抓。
      </p>
      <p className="muted hint">
        通過檢查<strong>不等於</strong>這句一定不難——只等於「我們量得到的那些都沒超標」。
        句型清單越完整，這句話才越有份量。
      </p>

      {error && <p className="error">{error}</p>}
    </section>
  );
}

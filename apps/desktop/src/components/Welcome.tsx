import { useEffect, useState } from "react";
import { currentLanguages, dictionaryStats, errorMessage, languageName } from "../api";

interface Props {
  /** 使用者按下「去匯入」時切到匯入頁 */
  onGoImport: () => void;
  /** 字典已經有東西時要顯示的內容 */
  children: React.ReactNode;
}

/** 一份字典的取得方式。指令要能直接複製貼上，不然等於沒寫。 */
interface Source {
  name: string;
  what: string;
  license: string;
  command: string;
}

const ENGLISH_SOURCES: Source[] = [
  {
    name: "ECDICT",
    what: "中文翻譯、音標、詞頻，還有國中會考／學測／多益等考試範圍標籤",
    license: "MIT",
    command:
      "curl -LO https://raw.githubusercontent.com/skywind3000/ECDICT/master/ecdict.csv\n" +
      "wordforge import ecdict ecdict.csv",
  },
  {
    name: "Wiktionary（kaikki.org）",
    what: "英文定義、例句、詞形變化表、發音。3.2 GB，可以之後再補",
    license: "CC BY-SA 4.0",
    command:
      "wget https://kaikki.org/dictionary/English/kaikki.org-dictionary-English.jsonl\n" +
      "wordforge import kaikki kaikki.org-dictionary-English.jsonl",
  },
];

/**
 * 第一次開啟的引導。
 *
 * 沒有這一層的話，新使用者看到的是「牌組是空的，到牌組頁加入單字」，
 * 但牌組頁也是空的——因為根本還沒有字典。那是一條死路，而且完全看不出
 * 問題在哪。裝了打不開的 App 只會被刪掉。
 *
 * 這個畫面只在**資料庫裡一個詞條都沒有**時出現，所以老使用者永遠看不到。
 */
export default function Welcome({ onGoImport, children }: Props) {
  const [empty, setEmpty] = useState<boolean | null>(null);
  const [target, setTarget] = useState("en");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void (async () => {
      try {
        const [stats, langs] = await Promise.all([dictionaryStats(), currentLanguages()]);
        setEmpty(stats.lemmas === 0);
        setTarget(langs.target);
      } catch (e) {
        setError(errorMessage(e));
        // 查不到就當作有字典，至少不要把正常畫面擋掉
        setEmpty(false);
      }
    })();
  }, []);

  // 還在查的時候不要閃一下引導畫面再跳回去
  if (empty === null) {
    return null;
  }
  if (!empty) {
    return <>{children}</>;
  }

  const isEnglish = target.split("-")[0].toLowerCase() === "en";

  return (
    <section className="panel welcome">
      <h2>先匯入一份字典</h2>
      <p>
        Wordforge <strong>不內建任何字典資料</strong>
        ——好用的字典幾乎都有版權，散布出去是侵權。 所以第一步是匯入一份你自己取得的字典，之後
        <strong>單字、發音、例句、考試範圍</strong>都從它來。
      </p>
      <p className="muted">
        這也是為什麼換一份字典就能學另一種語言：程式本身不預設你在學什麼。
        目前設定的目標語言是<strong>{languageName(target)}</strong>
        （可以到設定頁改）。
      </p>

      <ol className="steps">
        <li>
          <strong>匯入字典</strong>
          <p className="muted">下面任選一份，用命令列或「匯入」頁都可以</p>
        </li>
        <li>
          <strong>做分級測驗</strong>
          <p className="muted">在「牌組」頁，估出你的程度，已經會的字就不會再排給你</p>
        </li>
        <li>
          <strong>開始複習</strong>
          <p className="muted">想用 AI 出題的話，到設定頁選一個後端</p>
        </li>
      </ol>

      {isEnglish ? (
        <>
          <h3>學英文的話，這兩份最實用</h3>
          {ENGLISH_SOURCES.map((s) => (
            <div key={s.name} className="source">
              <p>
                <strong>{s.name}</strong>
                <span className="tag">{s.license}</span>
              </p>
              <p className="muted hint">{s.what}</p>
              <pre>{s.command}</pre>
            </div>
          ))}
          <p className="muted hint">
            兩份可以並存：ECDICT 給你中文翻譯與考試範圍，Wiktionary 補上英文定義與例句。
            只想先跑起來的話，匯入 ECDICT 就夠了。
          </p>
        </>
      ) : (
        <>
          <h3>其他語言</h3>
          <p className="muted">
            kaikki.org 有一百多種語言的 Wiktionary 資料，都是同一種 JSONL 格式。
            到下面的網址找到你要的語言，下載之後用「匯入」頁載入即可。
          </p>
          <pre>https://kaikki.org/</pre>
          <p className="muted hint">
            匯入時記得把語言設成 <code>{target}</code>，跟設定頁的目標語言一致，
            否則查字典跟出題都會對不上。
          </p>
        </>
      )}

      <button className="primary" onClick={onGoImport}>
        去匯入頁
      </button>

      {error && <p className="error">{error}</p>}
    </section>
  );
}

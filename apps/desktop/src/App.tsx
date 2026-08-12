import { useState } from "react";
import Deck from "./pages/Deck";
import Dictionary from "./pages/Dictionary";
import Import from "./pages/Import";
import Practice from "./pages/Practice";
import Review from "./pages/Review";
import Settings from "./pages/Settings";
import Welcome from "./components/Welcome";

const TABS = [
  { id: "review", label: "複習" },
  { id: "dictionary", label: "字典" },
  { id: "practice", label: "練習" },
  { id: "deck", label: "牌組" },
  { id: "import", label: "匯入" },
  { id: "settings", label: "設定" },
] as const;

type Tab = (typeof TABS)[number]["id"];

export default function App() {
  const [tab, setTab] = useState<Tab>("review");
  // 練習頁進去過就不再卸載。出一題要幾十秒，中途切去查個字回來
  // 就發現題目不見了——不是模型被中斷，是 React 把元件連同
  // 那個還沒 resolve 的 promise 一起丟掉了。
  const [practiceVisited, setPracticeVisited] = useState(false);
  if (tab === "practice" && !practiceVisited) {
    setPracticeVisited(true);
  }

  return (
    <div className="app">
      <header className="topbar">
        <h1>Wordforge</h1>
        <nav className="tabs">
          {TABS.map((t) => (
            <button
              key={t.id}
              className={tab === t.id ? "tab active" : "tab"}
              onClick={() => setTab(t.id)}
            >
              {t.label}
            </button>
          ))}
        </nav>
      </header>

      <main className="page">
        {/* 資料庫一個詞條都沒有時，複習頁換成引導——那是預設落點，
            也是新使用者唯一會看到的地方 */}
        {tab === "review" && (
          <Welcome onGoImport={() => setTab("import")}>
            <Review />
          </Welcome>
        )}
        {tab === "dictionary" && <Dictionary />}
        {practiceVisited && (
          <div hidden={tab !== "practice"}>
            <Practice />
          </div>
        )}
        {tab === "deck" && <Deck />}
        {tab === "import" && <Import />}
        {tab === "settings" && <Settings />}
      </main>
    </div>
  );
}

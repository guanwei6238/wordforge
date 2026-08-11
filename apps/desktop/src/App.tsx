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
        {tab === "practice" && <Practice />}
        {tab === "deck" && <Deck />}
        {tab === "import" && <Import />}
        {tab === "settings" && <Settings />}
      </main>
    </div>
  );
}

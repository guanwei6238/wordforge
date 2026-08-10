import { useState } from "react";
import Deck from "./pages/Deck";
import Dictionary from "./pages/Dictionary";
import Import from "./pages/Import";
import Review from "./pages/Review";

const TABS = [
  { id: "review", label: "複習" },
  { id: "dictionary", label: "字典" },
  { id: "deck", label: "牌組" },
  { id: "import", label: "匯入" },
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
        {tab === "review" && <Review />}
        {tab === "dictionary" && <Dictionary />}
        {tab === "deck" && <Deck />}
        {tab === "import" && <Import />}
      </main>
    </div>
  );
}

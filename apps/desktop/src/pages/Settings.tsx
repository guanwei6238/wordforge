import { useCallback, useEffect, useState } from "react";
import { errorMessage, getStudySettings, type StudySettings, updateStudySettings } from "../api";
import LanguageSettings from "../components/LanguageSettings";
import MaterialManager from "../components/MaterialManager";
import UsageStats from "../components/UsageStats";
import LlmSetup from "../components/LlmSetup";

/** 閱讀覆蓋率的選項。 */
const COVERAGE_CHOICES = [
  { value: 0.9, label: "90%", hint: "每篇都有不少生字，讀起來要花力氣但學得多" },
  { value: 0.96, label: "96%", hint: "平衡點：讀得動，又每篇都有幾個新字" },
  { value: 0.98, label: "98%", hint: "幾乎全部看得懂，適合想練流暢度而不是擴充詞彙" },
];

/** 目標留存率的選項。數字背後的意義比數字本身重要，所以每個都附說明。 */
const RETENTION_CHOICES = [
  { value: 0.85, label: "85%", hint: "複習量少，但忘記的字會明顯變多" },
  { value: 0.9, label: "90%", hint: "平衡點，大多數人適用" },
  { value: 0.95, label: "95%", hint: "記得很牢，複習量大約多五成" },
];

/**
 * 學習設定。
 *
 * 這三個數字都會直接改變每天要花多少時間，所以旁邊都寫清楚代價是什麼——
 * 「每日新卡 50」聽起來很勤奮，但那代表每天一小時以上，而且三週後
 * 複習量會堆到你不想打開 App。
 */
export default function Settings() {
  const [settings, setSettings] = useState<StudySettings | null>(null);
  const [saved, setSaved] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setSettings(await getStudySettings());
    } catch (e) {
      setError(errorMessage(e));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  async function save(next: StudySettings) {
    setSettings(next);
    setError(null);
    try {
      // 後端會把超出範圍的值夾住，用回傳值覆蓋才是真正存下來的
      const stored = await updateStudySettings(next);
      setSettings(stored);
      setSaved("已儲存");
      setTimeout(() => setSaved(null), 1500);
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  if (!settings) {
    return <p className="muted">載入中…</p>;
  }

  return (
    <div className="settings">
      <LanguageSettings />

      <section className="panel">
        <h2>每日份量</h2>

        <label>
          每天引入新字
          <input
            type="number"
            min={0}
            max={500}
            value={settings.new_per_day}
            onChange={(e) => save({ ...settings, new_per_day: Number(e.target.value) })}
          />
          <span className="muted">張</span>
        </label>
        <p className="muted hint">
          每張新卡當天要按兩三次才會畢業，之後幾天還會回來找你。
          15 張大約是每天 10 分鐘；設 50 張的話，三週後每天的複習量會超過一小時。
          設 0 就是暫時只複習不學新字。
        </p>

        <label>
          每天最多複習
          <input
            type="number"
            min={10}
            max={9999}
            value={settings.max_reviews_per_day}
            onChange={(e) => save({ ...settings, max_reviews_per_day: Number(e.target.value) })}
          />
          <span className="muted">張</span>
        </label>
        <p className="muted hint">
          放假幾天回來會累積一堆到期的卡，這個上限讓你不會一打開就看到七百張。
          沒做完的會留到明天。
        </p>
      </section>

      <section className="panel">
        <h2>記憶目標</h2>
        <label>
          複習時希望有多少機率想得起來
          <select
            value={settings.desired_retention}
            onChange={(e) => save({ ...settings, desired_retention: Number(e.target.value) })}
          >
            {RETENTION_CHOICES.map((c) => (
              <option key={c.value} value={c.value}>
                {c.label}
              </option>
            ))}
          </select>
        </label>
        <p className="muted hint">
          {RETENTION_CHOICES.find((c) => Math.abs(c.value - settings.desired_retention) < 0.001)
            ?.hint ?? "自訂值"}
          。這個數字直接決定 FSRS 排出來的間隔——調高不是「學得更好」，
          而是拿更多複習時間換更少的遺忘。考試前調高、長期維持調低。
        </p>
      </section>

      <section className="panel">
        <h2>閱讀難度</h2>
        <label>
          文章裡你看得懂的字要占
          <select
            value={settings.reading_coverage}
            onChange={(e) => save({ ...settings, reading_coverage: Number(e.target.value) })}
          >
            {COVERAGE_CHOICES.map((c) => (
              <option key={c.value} value={c.value}>
                {c.label}
              </option>
            ))}
          </select>
        </label>
        <p className="muted hint">
          {COVERAGE_CHOICES.find((c) => Math.abs(c.value - settings.reading_coverage) < 0.001)
            ?.hint ?? "自訂值"}
          。生詞的數量由這個值反推：300 字的文章設 96% 大約放 6 個新字，
          設 90% 會放 15 個左右。
        </p>
        <p className="muted hint">
          文章裡還會另外帶進幾個你<strong>學過但快忘掉</strong>的字（依 FSRS
          算出來的遺忘程度挑）。那些不算生詞，是免費的複習——在句子裡再遇到一次，
          比抽卡更接近真正的使用場景。
        </p>
      </section>

      <MaterialManager />

      <LlmSetup />

      <UsageStats />

      {saved && <p className="ok">{saved}</p>}
      {error && <p className="error">{error}</p>}
    </div>
  );
}

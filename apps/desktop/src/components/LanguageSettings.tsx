import { useCallback, useEffect, useState } from "react";
import {
  currentLanguages,
  dictionaryLanguages,
  errorMessage,
  languageName,
  type ProfileLanguages,
  setProfileLanguages,
  suspendOtherLanguageCards,
} from "../api";

/**
 * 母語的選項。
 *
 * 這個是「用什麼語言解釋給你聽」——字義翻譯、批改講評、文法說明都用它，
 * 所以清單短是刻意的：只列真的有人會拿來當解釋語言的。
 */
const NATIVE_CHOICES = ["zh-TW", "zh-CN", "en", "ja"];

/**
 * 學習語言設定。
 *
 * 目標語言的選項直接來自「你匯入了哪些字典」，不是一份寫死的清單——
 * 這個 App 的前提就是字典決定了你能學什麼，選單裡出現一個沒有字典的
 * 語言只會讓使用者選下去然後看到空白。
 */
export default function LanguageSettings() {
  const [langs, setLangs] = useState<ProfileLanguages | null>(null);
  const [available, setAvailable] = useState<[string, number][]>([]);
  const [target, setTarget] = useState("");
  const [native, setNative] = useState("");
  const [leftover, setLeftover] = useState<number | null>(null);
  const [saved, setSaved] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      const [current, langsInDict] = await Promise.all([currentLanguages(), dictionaryLanguages()]);
      setLangs(current);
      setTarget(current.target);
      setNative(current.native);
      setAvailable(langsInDict);
    } catch (e) {
      setError(errorMessage(e));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const dirty = !!langs && (target !== langs.target || native !== langs.native);

  async function save() {
    setBusy(true);
    setError(null);
    setSaved(null);
    try {
      const changed = await setProfileLanguages(native, target);
      setLangs(changed.languages);
      setLeftover(changed.other_language_cards || null);
      setSaved("已儲存");
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  async function tidyDeck() {
    setBusy(true);
    setError(null);
    try {
      const n = await suspendOtherLanguageCards();
      setLeftover(null);
      setSaved(`已收起 ${n} 張`);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  if (!langs) {
    return null;
  }

  // 目標語言可能是還沒匯入字典的語言（例如剛裝好的預設值），也要列出來
  const targetChoices = available.some(([code]) => code === langs.target)
    ? available
    : [...available, [langs.target, 0] as [string, number]];

  return (
    <section className="panel">
      <h2>學習語言</h2>

      <label>
        我要學
        <select value={target} onChange={(e) => setTarget(e.target.value)} disabled={busy}>
          {targetChoices.map(([code, count]) => (
            <option key={code} value={code}>
              {languageName(code)}（{code}）
              {count > 0 ? ` · ${count.toLocaleString()} 詞` : " · 尚未匯入字典"}
            </option>
          ))}
        </select>
      </label>
      <p className="muted hint">
        選項來自你匯入的字典。想學清單上沒有的語言，到「匯入」頁載入那個語言的字典
        （例如 kaikki.org 的 JSONL）就會出現在這裡。
      </p>

      <label>
        用這個語言解釋
        <select value={native} onChange={(e) => setNative(e.target.value)} disabled={busy}>
          {NATIVE_CHOICES.map((code) => (
            <option key={code} value={code}>
              {languageName(code)}（{code}）
            </option>
          ))}
        </select>
      </label>
      <p className="muted hint">
        出題、批改講評、文法說明都會用這個語言寫。翻譯練習的方向也跟著它走。
      </p>

      <button onClick={save} disabled={!dirty || busy}>
        {busy ? "處理中…" : "儲存"}
      </button>

      {leftover !== null && (
        <div className="warning">
          <p>
            牌組裡還有 <strong>{leftover.toLocaleString()}</strong> 張別的語言的卡片。
            不處理的話它們還是會混在每天的複習裡。
          </p>
          <button onClick={tidyDeck} disabled={busy}>
            把它們收起來
          </button>
          <p className="muted hint">收起來不是刪除——換回原本的語言時，複習歷史都還在。</p>
        </div>
      )}

      {saved && <p className="ok">{saved}</p>}
      {error && <p className="error">{error}</p>}
    </section>
  );
}

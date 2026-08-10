import { useState } from "react";
import { speak } from "../api";

interface Props {
  text: string;
  lang?: string;
  /** 有真人錄音時的相對路徑。目前尚未實作播放，先用來標示音源。 */
  audioPath?: string | null;
}

/**
 * 發音按鈕。
 *
 * 目前一律走系統語音合成——真人錄音要先做下載器與快取，
 * 那是獨立的一段工作。合成音對「這個字大概怎麼唸」夠用，
 * 但對細部發音不夠，所以之後真人音檔會蓋過它。
 */
export default function SpeakButton({ text, lang = "en", audioPath }: Props) {
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState<string | null>(null);

  async function play() {
    setBusy(true);
    setFailed(null);
    try {
      await speak(text, lang);
    } catch (e) {
      // 沒安裝語音引擎是最常見的原因，錯誤訊息本身已經寫了怎麼裝
      setFailed(typeof e === "object" && e && "message" in e ? String(e.message) : String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <button
        className="speak"
        onClick={play}
        disabled={busy}
        title={audioPath ? "播放發音" : "以系統語音合成朗讀"}
        aria-label={`朗讀 ${text}`}
      >
        {busy ? "🔈" : "🔊"}
      </button>
      {failed && <span className="error speak-error">{failed}</span>}
    </>
  );
}

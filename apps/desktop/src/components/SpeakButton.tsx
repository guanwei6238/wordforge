import { useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { audioFilePath, errorMessage, speak } from "../api";

interface Props {
  text: string;
  lang?: string;
  /** 已下載的真人錄音檔名（相對於 app 的 audio 目錄） */
  audioPath?: string | null;
}

/**
 * 發音按鈕。
 *
 * 優先播放真人錄音，沒有才退回系統語音合成。合成音判斷「大概怎麼唸」夠用，
 * 但重音與連音只有真人錄音聽得出來，所以順序不能反過來。
 */
export default function SpeakButton({ text, lang, audioPath }: Props) {
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState<string | null>(null);

  async function play() {
    setBusy(true);
    setFailed(null);
    try {
      if (audioPath) {
        const src = convertFileSrc(await audioFilePath(audioPath));
        const audio = new Audio(src);
        // 播放失敗（檔案損毀、格式不支援）就退回合成音，總比沒聲音好
        await audio.play().catch(async () => {
          await speak(text, lang);
        });
      } else {
        await speak(text, lang);
      }
    } catch (e) {
      setFailed(errorMessage(e));
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
        title={audioPath ? "播放真人錄音" : "以系統語音合成朗讀（尚未下載真人錄音）"}
        aria-label={`朗讀 ${text}`}
      >
        {busy ? "🔈" : audioPath ? "🔊" : "🗣"}
      </button>
      {failed && <span className="error speak-error">{failed}</span>}
    </>
  );
}

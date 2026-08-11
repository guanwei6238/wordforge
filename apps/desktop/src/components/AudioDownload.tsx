import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  AUDIO_PROGRESS_EVENT,
  type AudioProgress,
  audioStatus,
  downloadAudio,
  errorMessage,
} from "../api";

/**
 * 下載牌組裡那些字的真人發音。
 *
 * 完整的 Wiktionary 音檔集有好幾 GB，但實際會聽到的只有牌組裡這幾百個字，
 * 所以只抓需要的——300 個字大約 10 MB。
 */
export default function AudioDownload() {
  const [available, setAvailable] = useState(0);
  const [downloaded, setDownloaded] = useState(0);
  const [progress, setProgress] = useState<AudioProgress | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const [a, d] = await audioStatus();
      setAvailable(a);
      setDownloaded(d);
    } catch (e) {
      setError(errorMessage(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
    const sub = listen<AudioProgress>(AUDIO_PROGRESS_EVENT, (e) => setProgress(e.payload));
    return () => {
      void sub.then((un) => un());
    };
  }, [refresh]);

  async function start() {
    setBusy(true);
    setError(null);
    setProgress(null);
    try {
      const result = await downloadAudio();
      setProgress(result);
      await refresh();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  const pending = available - downloaded;

  return (
    <section className="panel">
      <h2>真人發音</h2>

      {available === 0 ? (
        <p className="muted">
          牌組裡的字都沒有可用的錄音。Wiktionary 大約四成的詞條有真人錄音，
          先匯入它再回來看看。目前會用系統語音合成代替。
        </p>
      ) : (
        <>
          <p className="muted">
            牌組裡有 <strong>{available.toLocaleString()}</strong> 個字有真人錄音，
            已下載 <strong>{downloaded.toLocaleString()}</strong> 個。
            {pending > 0 && `還有 ${pending.toLocaleString()} 個可以下載（約 ${Math.ceil(pending * 30 / 1024)} MB）。`}
          </p>
          <progress value={downloaded} max={available} />

          {progress && (
            <p className="muted">
              本次下載 {progress.downloaded} / {progress.total}
              {progress.failed > 0 && `，失敗 ${progress.failed}`}
            </p>
          )}

          <div className="row">
            <button className="primary" onClick={start} disabled={busy || pending === 0}>
              {busy ? "下載中…" : pending === 0 ? "全部已下載" : "下載發音"}
            </button>
          </div>
          <p className="muted">
            音檔來自 Wikimedia Commons，授權多為 CC BY-SA 或 CC0，逐檔標示。
          </p>
        </>
      )}

      {error && <p className="error">{error}</p>}
    </section>
  );
}

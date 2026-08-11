-- 真人錄音的來源網址。
--
-- 匯入時只記網址不下載：一份完整的 Wiktionary 音檔集有好幾 GB，
-- 但學習者實際會用到的只有牌組裡那幾百個字。
-- 有了網址，之後可以隨時針對需要的字下載，不必重掃 3 GB 的原始檔。
ALTER TABLE pronunciation ADD COLUMN audio_url TEXT;

-- 下載器要找的就是「有網址、還沒下載」的那些
CREATE INDEX idx_pron_pending_audio ON pronunciation (lemma_id)
    WHERE audio_url IS NOT NULL AND audio_path IS NULL;

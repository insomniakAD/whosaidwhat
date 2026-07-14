// Recording pill. Elapsed time is kept client-side from the recording-state
// event (the backend has no per-second tick to push; a meeting recorder does
// not need frame-accurate elapsed display).

"use strict";

(async () => {
  const timeEl = document.getElementById("time");
  let startedAt = null;
  let timer = null;

  function render() {
    if (startedAt == null) return;
    timeEl.textContent = WSW.fmtTs(Date.now() - startedAt);
  }

  await WSW.listen("recording-state", (e) => {
    const rec = e.payload && e.payload.recording;
    if (rec) {
      startedAt = Date.now();
      clearInterval(timer);
      timer = setInterval(render, 500);
    } else {
      clearInterval(timer);
      startedAt = null;
      timeEl.textContent = "00:00";
    }
  });

  document.getElementById("stop").addEventListener("click", () => {
    WSW.invoke("stop_recording");
  });
})();

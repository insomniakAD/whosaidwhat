// Consent prompt window. The copy arrives via the "meeting-detected" event
// (same wording as notify::prompt_copy on the Rust side); the two buttons
// answer through the prompt_response command, which hides this window and
// forwards the choice to the SessionManager.

"use strict";

(async () => {
  const titleEl = document.getElementById("title");
  const bodyEl = document.getElementById("body");

  await WSW.listen("meeting-detected", (e) => {
    if (e.payload) {
      titleEl.textContent = e.payload.title || "Meeting detected";
      bodyEl.textContent = e.payload.body || "";
    }
  });

  document.getElementById("record").addEventListener("click", () => {
    WSW.invoke("prompt_response", { accept: true });
  });
  document.getElementById("ignore").addEventListener("click", () => {
    WSW.invoke("prompt_response", { accept: false });
  });
})();

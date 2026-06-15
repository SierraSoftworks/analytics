// Detect a user opt-out from tracking. We honour both the legacy Do-Not-Track
// signal and the newer Global Privacy Control. When any of these is set the tracker
// collects nothing at all (the server honours these too, as a second line of
// defence). `navigator.doNotTrack` can be "1"/"0"/"yes"/"unspecified"/null depending
// on the browser; `window.doNotTrack` is the old IE/Edge location.
export function privacySignal(nav, win) {
  if (!nav) return false;
  return (
    nav.doNotTrack === "1" ||
    nav.doNotTrack === "yes" ||
    (win && win.doNotTrack === "1") ||
    nav.globalPrivacyControl === true
  );
}

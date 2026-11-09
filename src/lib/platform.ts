/**
 * Whether NEXUS is running as the Android app.
 *
 * The WebView's user agent says so, which is known before the first render
 * and needs no call to the backend. The phone shows its own shell with only
 * the screens that work there; see docs/MOBILE_APP.md.
 */
export const IS_PHONE = /Android/i.test(navigator.userAgent);

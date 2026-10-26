/**
 * NEXUS bank SMS inbox.
 *
 * The phone's SMS forwarder POSTs each ICICI or HDFC message here; it is
 * stored as a row in a Google Sheet in your own account. NEXUS reads the rows
 * back with GET. Both directions need the secret key, so the web app URL alone
 * is not enough to read or write anything.
 *
 * Setup: run `setup` once from the editor, then Deploy > New deployment >
 * Web app, Execute as: Me, Who has access: Anyone.
 */

const SHEET_NAME = 'Bank SMS';
const BANK_SENDER = /ICICI|HDFC/i;
// A second line of defence behind the phone's own filter: one-time codes and
// login alerts are never stored, even if the phone rule is misconfigured.
const NEVER_STORE = /\bOTP\b|one[\s-]?time\s+pass|verification\s+code|do not share/i;
const MAX_ROWS_PER_READ = 500;

function setup() {
  const props = PropertiesService.getScriptProperties();
  if (!props.getProperty('SHEET_ID')) {
    const book = SpreadsheetApp.create('NEXUS Bank SMS');
    const sheet = book.getSheets()[0];
    sheet.setName(SHEET_NAME);
    sheet.appendRow(['Received', 'From', 'Message', 'Sent (ms)', 'Id']);
    sheet.setFrozenRows(1);
    props.setProperty('SHEET_ID', book.getId());
  }
  if (!props.getProperty('KEY')) {
    props.setProperty('KEY', (Utilities.getUuid() + Utilities.getUuid()).replace(/-/g, ''));
  }
  Logger.log('Sheet: https://docs.google.com/spreadsheets/d/' + props.getProperty('SHEET_ID'));
  Logger.log('Secret key: ' + props.getProperty('KEY'));
}

function keyOk_(e) {
  const key = PropertiesService.getScriptProperties().getProperty('KEY');
  return Boolean(key) && e && e.parameter && e.parameter.k === key;
}

function json_(value) {
  return ContentService.createTextOutput(JSON.stringify(value))
    .setMimeType(ContentService.MimeType.JSON);
}

function sheet_() {
  const id = PropertiesService.getScriptProperties().getProperty('SHEET_ID');
  return SpreadsheetApp.openById(id).getSheetByName(SHEET_NAME);
}

/** From the phone: store one bank message. */
function doPost(e) {
  if (!keyOk_(e)) return json_({ ok: false, error: 'key' });

  let body;
  try {
    body = JSON.parse(e.postData.contents);
  } catch (err) {
    return json_({ ok: false, error: 'json' });
  }
  const from = String(body.from || '').slice(0, 64);
  const text = String(body.text || '').slice(0, 1000);
  const sent = Number(body.sentStamp) || 0;

  if (!BANK_SENDER.test(from)) return json_({ ok: true, skipped: 'sender' });
  if (NEVER_STORE.test(text)) return json_({ ok: true, skipped: 'otp' });

  // The forwarder retries on a failed request, so the same SMS can arrive
  // twice. Remember each one for six hours and store it once.
  const id = Utilities.base64EncodeWebSafe(
    Utilities.computeDigest(Utilities.DigestAlgorithm.SHA_256, from + '|' + sent + '|' + text)
  ).slice(0, 22);
  const cache = CacheService.getScriptCache();
  if (cache.get(id)) return json_({ ok: true, skipped: 'duplicate' });

  const lock = LockService.getScriptLock();
  lock.waitLock(10000);
  try {
    sheet_().appendRow([new Date(), from, text, sent, id]);
    cache.put(id, '1', 21600);
  } finally {
    lock.releaseLock();
  }
  return json_({ ok: true });
}

/** From NEXUS: rows after `after` (a row number), oldest first. */
function doGet(e) {
  if (!keyOk_(e)) return json_({ ok: false, error: 'key' });

  const sheet = sheet_();
  const last = sheet.getLastRow();
  const after = Math.max(1, Number(e.parameter.after) || 1);
  if (last <= after) return json_({ ok: true, last: last, rows: [] });

  const count = Math.min(last - after, MAX_ROWS_PER_READ);
  const values = sheet.getRange(after + 1, 1, count, 5).getValues();
  const rows = values.map(function (v, i) {
    return {
      row: after + 1 + i,
      received: v[0] instanceof Date ? v[0].getTime() : null,
      from: String(v[1]),
      text: String(v[2]),
      sent: Number(v[3]) || null,
      id: String(v[4]),
    };
  });
  return json_({ ok: true, last: after + count, rows: rows });
}

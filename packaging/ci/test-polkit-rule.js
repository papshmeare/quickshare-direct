// Evaluates packaging/linux/polkit/50-quickshare-direct.rules with stub polkit objects:
// syntax plus the allow/deny decisions (node packaging/ci/test-polkit-rule.js).
const fs = require("fs");
const path = require("path");
const rule = fs.readFileSync(path.join(__dirname, "../linux/polkit/50-quickshare-direct.rules"), "utf8");
const Result = { YES: "yes", NO: "no", AUTH_ADMIN: "auth_admin", NOT_HANDLED: undefined };
let fn;
new Function("polkit", rule)({ Result, addRule: (f) => { fn = f; } });
const action = (id, unit, verb) => ({ id, lookup: (k) => ({ unit, verb })[k] });
const subject = (user, active = true, local = true) => ({ user, active, local });
const M = "org.freedesktop.systemd1.manage-units";
const cases = [
  [action(M, "quickshare-ap@alice.service", "start"), subject("alice"), "yes"],
  [action(M, "quickshare-join@alice.service", "stop"), subject("alice"), "yes"],
  [action(M, "quickshare-ap@bob.service", "start"), subject("alice"), undefined],
  [action(M, "quickshare-ap@alice.service", "start"), subject("alice", false), undefined],
  [action(M, "quickshare-ap@alice.service", "start"), subject("alice", true, false), undefined],
  [action(M, "sshd.service", "start"), subject("alice"), undefined],
  [action(M, "quickshare-ap@alice.service", "enable"), subject("alice"), undefined],
  [action("org.freedesktop.login1.reboot", undefined, undefined), subject("alice"), undefined],
];
let bad = 0;
for (const [a, s, want] of cases) {
  const got = fn(a, s);
  if (got !== want) { bad++; console.log(`FAIL ${a.id} ${a.lookup("unit")} ${a.lookup("verb")} as ${s.user}: ${got} != ${want}`); }
}
console.log(bad ? `${bad} failures` : `polkit rule OK (${cases.length} cases)`);
process.exit(bad ? 1 : 0);

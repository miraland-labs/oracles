# Oracle Intro Video — Narration Script

**Total runtime:** ~110 seconds (target: under 2 minutes).
**Tone:** confident, friendly, builder-to-builder. Plain English; no marketing fluff.
**Visual cadence:** suggested on-screen captions / B-roll cues are in `[brackets]`.

---

## [0:00 – 0:12] · Hook (12s)

> *[Visual: split screen — left a buyer agent, right a seller agent; chevrons of payment moving between them]*

**Narrator:**
"AI agents are paying each other in real-time on Solana. But what happens
when one of them owes work that takes minutes, hours, or days to deliver?
**That's where oracles come in.**"

> *[Visual: zoom into an `sla-escrow` payment locking funds on-chain]*

---

## [0:12 – 0:35] · What an oracle does (23s)

> *[Visual: three-step diagram — buyer funds escrow → seller delivers → oracle adjudicates]*

**Narrator:**
"x402 has two settlement rails on Solana. The **`exact`** rail is for instant
micro-payments. The **`sla-escrow`** rail locks funds while the seller
works — and pays out **only** when an oracle confirms delivery."

> *[Visual: caption — "Oracles = the trust layer for asynchronous machine commerce"]*

"An oracle is a small standalone service. It watches the chain for
delivery events, fetches the seller's evidence, checks it against the
buyer's SLA, and submits a verdict on-chain. **One verdict, deterministic,
auditable, paid as a tip whether it approves or rejects.**"

---

## [0:35 – 1:05] · The whole oracle process (30s)

> *[Visual: animated flow with five labeled steps]*

**Narrator:**
"The flow is five steps."

> *[Visual: step 1 lights up — "1. Watch"]*

"**One.** The oracle subscribes to the SLA-Escrow program over WebSocket
and waits for `DeliverySubmittedEvent`."

> *[Visual: step 2 — "2. Fetch"]*

"**Two.** It fetches the SLA and the delivery evidence from a
content-addressed registry — every byte verified by SHA-256 before it's
parsed."

> *[Visual: step 3 — "3. Evaluate"]*

"**Three.** It runs the profile-specific checks. JSON quality. SPL
transfer delta. Streamed file size and MIME. **One profile, one binary,
one canonical rule set.**"

> *[Visual: step 4 — "4. Settle"]*

"**Four.** It submits `ConfirmOracle` on-chain with a verdict — approved
or rejected — plus a deterministic resolution hash. **Anyone can recompute
that hash and verify the verdict.**"

> *[Visual: step 5 — "5. Get paid"]*

"**Five.** The oracle earns a **verdict-neutral tip** — paid for
adjudicating, not for the outcome. Default 50 basis points."

---

## [1:05 – 1:35] · How to become an oracle developer (30s)

> *[Visual: GitHub repo URL on screen — `github.com/miraland-labs/oracles`]*

**Narrator:**
"Becoming an oracle developer is genuinely simple."

> *[Visual: caption — "1. Clone the closest sibling"]*

"**One.** Clone the closest reference oracle. We ship three: api-quality
for JSON responses, onchain-transfer for SPL deliveries, file-delivery for
large files. Pick the one closest to your domain."

> *[Visual: caption — "2. Swap the evaluator"]*

"**Two.** Implement the `OracleEvaluator` trait. Two methods. Your domain
expertise goes here — and only here."

> *[Visual: caption — "3. Register your profile id"]*

"**Three.** Pick a profile id like `x402/oracles/<your-domain>/v1` and
register it once at startup."

> *[Visual: caption — "4. Run install.sh"]*

"**Four.** Run our installer on Ubuntu 24.04. One command. Postgres and
MinIO bootstrap scripts included."

> *[Visual: caption — "5. Get advertised"]*

"**Five.** The facilitator reviews and endorses your oracle, then lists
it on `GET /capabilities`. Sellers reference you in their HTTP-402
challenge. Buyers pick you. You earn tips on every verdict."

---

## [1:35 – 1:50] · How sellers submit deliveries (15s)

> *[Visual: terminal pane showing three curl commands]*

**Narrator:**
"And for sellers integrating with you: it's three curl calls. POST the
SLA. POST the delivery. Submit the on-chain hash. **That's the entire
loop.** Documented in our seller guide."

---

## [1:50 – 2:00] · Close (10s)

> *[Visual: ecosystem map — pr402, sla-escrow, three oracle siblings, with the oracle box highlighted]*

**Narrator:**
"Oracles are how machine commerce earns trust on Solana. Bring your
domain expertise. We brought the rails."

> *[Visual: end card — `github.com/miraland-labs/oracles` · `oracles/docs/SELLER_GUIDE.md` · `oracles/docs/BUYER_GUIDE.md`]*

**Narrator (voiceover):**
"Start at `github.com/miraland-labs/oracles`."

---

## Production notes

| Element                | Recommendation                                                                    |
| ---------------------- | --------------------------------------------------------------------------------- |
| **Voice**              | Calm, mid-tempo. No hype voice. Think Linus Torvalds talking about kernel design. |
| **Music**              | Sparse synth bed; cuts out at 1:35 to let the seller-flow CTA breathe.            |
| **On-screen code**     | Real `cargo` commands and real curl from `oracles/docs/SELLER_GUIDE.md` §4.       |
| **B-roll**             | A live `journalctl -u oracle@api-quality.service -f` panel during steps 1 and 2.  |
| **Color**              | Match `pr402/public/index.html` accent palette so video and homepage feel related.|
| **Captions**           | Always-on. Many viewers watch agent / dev content muted at first.                 |
| **Length discipline**  | Cut anything that drifts past 1:55. Brevity is the whole pitch.                   |

## Word count

- Body narration: ~310 words
- At a comfortable 165 wpm: ~112 seconds spoken — fits the 2-minute window
  with a 5–8 second buffer for transition beats.

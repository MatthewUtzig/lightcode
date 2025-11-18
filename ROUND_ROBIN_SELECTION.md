# Round-Robin Account Selection and Limits UI

This note codifies how the CLI should choose between multiple authenticated
accounts ("slots"), how we compute instantaneous selection chance, and how the
limits UI surfaces that information. The goals are to keep usage roughly even
across accounts, bias toward connections with spare quota, suppress exhausted
slots immediately, and give the user a transparent view of the scheduler.

---

## Data Model

- **Account** – concrete credential used for requests.
  - `account_id`, `display_name`, `enabled`, `health_status`
    (`Healthy`, `TemporarilyUnavailable`, `HardError`).
  - Quota metadata: `weekly_limit_tokens`, `weekly_used_tokens`,
    `weekly_window_start`, `weekly_window_end` (UTC timestamps). Optional
    hourly limits follow the same shape.
- **Slot** – configuration entry that points at exactly one account; a single
  account may appear in multiple slots. Each slot has an optional
  `base_slot_weight` (defaults to `1.0`).

All metrics are recomputed every time we schedule a request or update the
limits overlay.

---

## Core Ratios

1. **Week progress**
   - `week_elapsed = clamp(now - weekly_window_start, 0, week_length)`.
   - `week_left = week_length - week_elapsed`.
   - `%week_left = max(week_left / week_length, ε)` where ε prevents division
     by zero.
2. **Quota remaining**
   - `tokens_remaining = max(weekly_limit_tokens - weekly_used_tokens, 0)`.
   - `%weekly_limit_remaining = tokens_remaining / weekly_limit_tokens`
     (accounts with unknown limits are treated as "unbounded").
3. **Usage ratio**
   - `ratio = %weekly_limit_remaining / %week_left` (for bounded accounts).
   - `ratio > 1` → under-used (safe to upweight). `ratio < 1` → over-used
     (downweight to avoid early exhaustion).

---

## Urgency Multipliers

Define tunable constants (defaults shown):

| Symbol | Default | Meaning |
| ------ | ------- | ------- |
| `R_CRITICAL` | 0.25 | strongly over-used |
| `R_LOW` | 1.0 | on-track lower bound |
| `R_SURPLUS` | 1.5 | surplus threshold |
| `R_CAP` | 4.0 | treat higher ratios equally |
| `U_MIN` | 0.1 | minimum multiplier |
| `U_BASE` | 1.0 | neutral multiplier |
| `U_MAX` | 2.0 | maximum multiplier |

Mapping:

- `ratio ≤ R_CRITICAL` ⇒ `U_MIN`.
- `R_CRITICAL < ratio < R_LOW` ⇒ interpolate between `U_MIN` and `U_BASE`.
- `R_LOW ≤ ratio < R_SURPLUS` ⇒ `U_BASE`.
- `R_SURPLUS ≤ ratio < R_CAP` ⇒ interpolate between `U_BASE` and `U_MAX`.
- `ratio ≥ R_CAP` ⇒ `U_MAX`.
- Unbounded accounts get `U_BASE` (optionally globally scaled).

Other multipliers:

- `health_multiplier` ∈ {`0.0` hard error, `0.2` temporary issue, `1.0`
  healthy}.
- Optional `hourly_multiplier` derived from hourly ratios.

---

## Slot Weights and Selection Chance

Per-slot final weight:

```
slot_weight = base_slot_weight
              * urgency_multiplier
              * health_multiplier
              * hourly_multiplier (optional)
```

Slots with `slot_weight ≤ 0` are excluded. For the remaining candidate set `C`:

```
total_weight = Σ slot_weight_i
selection_chance_slot_i = slot_weight_i / total_weight
selection_chance_account[a] = Σ selection_chance_slot_i (for slots owned by a)
```

### Scheduler

- Maintain smooth weighted round-robin state over the configured slots.
- Before each request: refresh metrics, rebuild `slot_weight`, drop disabled or
  exhausted slots, then run one iteration of smooth weighted RR using the
  weights above. The long-run frequency must match `selection_chance_slot_i`.
- After selection, record the slot index and update usage counters.

### Fallbacks

- If every slot weight collapses to zero, emit a scheduler error event:
  "No accounts available; all slots are exhausted or disabled."  The limits UI
  should highlight the failing accounts with `0% selection chance` and the next
  reset time if known.
- Accounts flagged as exhausted by the provider (e.g., API returns
  `rate_limit_exceeded`) are forced to `slot_weight = 0` until the next reset.
- Duplicate slots share the same account id; their weights add together and the
  UI must show both the per-slot chance and the combined account chance.

---

## Limits UI Integration

The `/limits` overlay and the condensed header should include:

1. **Selection Chance Line** – for each account:
   - `Selection chance: 64% (2 slots)` – aggregated value.
   - If any individual slot has a noticeably different chance, include a
     sub-line, e.g., `• Slot “api-key-2”: 40%`.
2. **Out-of-Tokens Warning** – when `slot_weight == 0` because of exhaustion:
   - Highlight the account in warning colors, show `0% selection chance`, and
   - Append `Out of tokens · resets in 2h 13m` (using the earliest reset).
3. **Duplicate Slot Notice** – if multiple slots point to the same account id:
   - Add a dimmed line `Duplicate slot configuration detected (3 slots)` so
     users understand why the selection chance is higher.
4. **Aggregate Tab** – show the aggregate rate-limit snapshot first, followed by
   the per-account tabs ordered the way we already do (active account, then the
   rest).

These additions keep the existing layout intact while making the scheduler
behaviour explicit to users.

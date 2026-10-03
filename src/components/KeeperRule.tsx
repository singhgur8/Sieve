// "Which photos count as keepers" rules and the menu that changes them (Cull summary, Edit plan, Export dialog).
import { ChevronDown } from "lucide-react";
import type { KeeperRule } from "../ipc";
import { Menu, menuItem } from "./Menu";
import { HelpLink } from "./HelpLink";

/**
 * Menu order is the array order, except that "Everything not rejected" (the default since IPC v18) is shown first.
 * Test ids follow the array index (`keeper-rule-0..3` are the pre-v18 rules, `keeper-rule-4` the default).
 */
export const KEEPER_RULES: { rule: KeeperRule; label: string; long: string }[] = [
  { rule: { mode: "picks_and_ratings", minRating: 1, useSuggestions: true }, label: "Picks, 1★+, suggested", long: "Picks, anything rated 1★ and up, and photos Sieve suggests" },
  { rule: { mode: "picks_and_ratings", minRating: 1, useSuggestions: false }, label: "Picks, 1★+", long: "Picks and anything rated 1★ and up" },
  { rule: { mode: "picks_and_ratings", minRating: 3, useSuggestions: false }, label: "Picks, 3★+", long: "Picks and photos rated 3★ and up" },
  { rule: { mode: "picks_and_ratings", minRating: 5, useSuggestions: false }, label: "Picks, 5★", long: "Picks and 5★ photos" },
  { rule: { mode: "not_rejected", minRating: 1, useSuggestions: true }, label: "Everything not rejected", long: "Everything you have not rejected (default)" },
];
/** Menu order of `KEEPER_RULES` indexes (default first). */
export const DISPLAY_ORDER = [4, 0, 1, 2, 3];

/** Same rule as far as keepers go (`not_rejected` ignores the thresholds). */
export const sameRule = (a: KeeperRule, b: KeeperRule) => a.mode === b.mode && (a.mode === "not_rejected" || (a.minRating === b.minRating && a.useSuggestions === b.useSuggestions));
export const ruleLabel = (r: KeeperRule) => KEEPER_RULES.find((x) => sameRule(x.rule, r))?.label ?? `${r.minRating}★+`;

export function RuleItems({ current, onPick }: { current: KeeperRule; onPick: (r: KeeperRule) => void }) {
  return (
    <div className="w-80 py-1" data-testid="keeper-rule-menu">
      <div className="flex justify-end px-3 pb-1"><HelpLink id="keepers" label="What are keepers?" /></div>
      {DISPLAY_ORDER.map((i) => {
        const k = KEEPER_RULES[i];
        const on = sameRule(k.rule, current);
        return (
          <button key={i} className={menuItem} data-testid={`keeper-rule-${i}`} aria-checked={on} role="menuitemradio" onClick={() => onPick(k.rule)}>
            <span className="w-4">{on ? "✓" : ""}</span>
            {k.long}
          </button>
        );
      })}
    </div>
  );
}

/** "Change keeper rule" link-style menu. */
export function KeeperRuleMenu({ current, onPick, testid = "keeper-rule-change", label = "Change keeper rule" }: { current: KeeperRule; onPick: (r: KeeperRule) => void; testid?: string; label?: string }) {
  return (
    <Menu
      trigger={
        <>
          {label} <ChevronDown className="size-3" />
        </>
      }
      triggerClass="flex items-center gap-0.5 whitespace-nowrap rounded px-1 text-xs text-sky-300 underline-offset-2 hover:underline"
      triggerTestId={testid}
      title={`Which photos go on to Edit and Export. Now: ${ruleLabel(current)}`}
    >
      {(close) => <RuleItems current={current} onPick={(r) => (close(), onPick(r))} />}
    </Menu>
  );
}

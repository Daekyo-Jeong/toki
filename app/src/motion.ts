/* Motion helpers — works on existing sprites without re-drawing them.
   - Breath bob: 1-sprite-pixel Y shift on idle, applied via CSS animation.
   - Blink frames for TEEN/ADULT/ELDER: rows 7-8 dots filled to close eyes.
   - Eating/sulk shake: tiny X jitter handled by CSS class. */

import {
  D_BABY,
  D_BABY_BLINK,
  D_BABY_EAT,
  D_BABY_SULK,
  D_BABY_SLEEP,
  D_EGG,
  D_TEEN,
  D_ADULT,
  D_ELDER,
  type Mode,
} from "./sprites";

/** Auto-generate a blink variant by replacing interior `.` dots in the
 *  eye rows (rows 7 and 8 by default) with `#`. Works for TEEN/ADULT/ELDER
 *  whose eyes are 1-pixel gaps inside a solid body row. */
function closeEyes(sprite: string, rowIndices: number[] = [7, 8]): string {
  const lines = sprite.replace(/^\n+|\n+$/g, "").split("\n");
  rowIndices.forEach((y) => {
    const row = lines[y];
    if (!row) return;
    // Replace interior `.` between two `#`s with `#`
    const chars = row.split("");
    for (let x = 1; x < chars.length - 1; x++) {
      if (chars[x] === "." && chars[x - 1] === "#" && chars[x + 1] === "#") {
        chars[x] = "#";
      }
    }
    lines[y] = chars.join("");
  });
  return "\n" + lines.join("\n") + "\n";
}

export const D_TEEN_BLINK = closeEyes(D_TEEN);
export const D_ADULT_BLINK = closeEyes(D_ADULT);
export const D_ELDER_BLINK = closeEyes(D_ELDER);

export type AnimFrames = { frames: string[]; ms: number };

/** Frame schedule for a (stage, mode) pair. null = static sprite. */
export function framesFor(stage: string, mode: Mode): AnimFrames | null {
  if (mode === "eating") {
    if (stage === "baby") return { frames: [D_BABY_EAT, D_BABY], ms: 220 };
    return null;
  }
  if (mode === "sulk") {
    if (stage === "baby") return { frames: [D_BABY_SULK], ms: 1000 };
    return null;
  }
  if (mode === "sleep") {
    if (stage === "baby") return { frames: [D_BABY_SLEEP], ms: 1000 };
    return null;
  }
  // idle blink — 3 idle frames + 1 blink frame, ~600ms per frame ≈ 2.4s cycle
  switch (stage) {
    case "egg":   return { frames: [D_EGG], ms: 1000 };
    case "baby":  return { frames: [D_BABY, D_BABY, D_BABY, D_BABY_BLINK], ms: 600 };
    case "teen":  return { frames: [D_TEEN, D_TEEN, D_TEEN, D_TEEN_BLINK], ms: 600 };
    case "adult": return { frames: [D_ADULT, D_ADULT, D_ADULT, D_ADULT_BLINK], ms: 700 };
    case "elder": return { frames: [D_ELDER, D_ELDER, D_ELDER, D_ELDER_BLINK], ms: 800 };
    default:      return null;
  }
}

/** CSS class names to apply to the sprite wrapper for ambient motion.
 *  Used for modes that DON'T walk — eating shake, sulk sway, sleep breath.
 *  `alive` mode uses JS-driven walking transform instead (see useWalk). */
export function motionClass(mode: Mode): string {
  switch (mode) {
    case "eating": return "motion-shake";
    case "sulk":   return "motion-sway";
    case "sleep":  return "motion-breath-slow";
    default:       return "";
  }
}


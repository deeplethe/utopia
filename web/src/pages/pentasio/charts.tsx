import { useEffect, useState } from "react";

/* A small integer counter for the live-run stats. The graph's tallies step in
   whole numbers, so this rounds; it honours reduced motion by landing on the
   final value immediately. Colour and type are left to the caller. */
export function CountUp({
  to,
  duration = 700,
}: {
  to: number;
  duration?: number;
}) {
  const [n, setN] = useState(0);

  useEffect(() => {
    if (
      typeof window !== "undefined" &&
      typeof window.matchMedia === "function" &&
      window.matchMedia("(prefers-reduced-motion: reduce)").matches
    ) {
      setN(to);
      return;
    }

    let raf = 0;
    const t0 = performance.now();
    const step = (t: number) => {
      const k = Math.min(1, (t - t0) / duration);
      setN(Math.round(to * (1 - Math.pow(1 - k, 3))));
      if (k < 1) raf = requestAnimationFrame(step);
    };
    raf = requestAnimationFrame(step);
    return () => cancelAnimationFrame(raf);
  }, [to, duration]);

  return <>{n}</>;
}

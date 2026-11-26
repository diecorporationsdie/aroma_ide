import type { ZTier } from '../types';

const TIER_BASE: Record<ZTier, number> = {
  background: 0,
  normal: 100,
  raised: 500,
  overlay: 1000,
  modal: 5000,
  system: 10000,
};

const TIER_STEP = 5;

export class ZIndexPool {
  private readonly counters = new Map<ZTier, number>();

  next(tier: ZTier): number {
    const current = this.counters.get(tier) ?? 0;
    this.counters.set(tier, current + 1);
    return TIER_BASE[tier] + current * TIER_STEP;
  }
}

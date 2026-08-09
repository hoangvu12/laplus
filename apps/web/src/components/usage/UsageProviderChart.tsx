import type { UsageBucket, UsageProviderKind } from "@t3tools/contracts";
import { useCallback, useMemo, useRef, useState } from "react";

import { PROVIDER_COLOR, PROVIDER_LABEL, PROVIDER_MARK, PROVIDER_ORDER } from "./usageProviders";

const VIEW_WIDTH = 960;
const VIEW_HEIGHT = 260;
const PLOT_TOP = 8;

export type UsageChartMetric = "tokens" | "cost";

export interface UsageProviderChartProps {
  readonly sinceDay: string;
  readonly untilDay: string;
  readonly buckets: readonly UsageBucket[];
  readonly metric: UsageChartMetric;
}

export interface DayColumn {
  readonly day: string;
  readonly bands: readonly { readonly provider: UsageProviderKind; readonly value: number }[];
  readonly total: number;
}

interface Point {
  readonly x: number;
  readonly y: number;
}

export function enumerateUsageDays(sinceDay: string, untilDay: string): readonly string[] {
  const start = Date.parse(`${sinceDay}T00:00:00Z`);
  const end = Date.parse(`${untilDay}T00:00:00Z`);
  if (Number.isNaN(start) || Number.isNaN(end) || end < start) return [];
  const days: string[] = [];
  for (let cursor = start; cursor <= end; cursor += 86_400_000) {
    days.push(new Date(cursor).toISOString().slice(0, 10));
  }
  return days;
}

function processedTokens(bucket: UsageBucket): number {
  return (
    bucket.totals.uncachedInputTokens +
    bucket.totals.cachedInputTokens +
    bucket.totals.cacheCreationTokens +
    bucket.totals.outputTokens
  );
}

export function buildDayColumns(
  days: readonly string[],
  buckets: readonly UsageBucket[],
  metric: UsageChartMetric,
): readonly DayColumn[] {
  const values = new Map<string, number>();
  for (const bucket of buckets) {
    const key = `${bucket.day}\0${bucket.provider}`;
    values.set(
      key,
      (values.get(key) ?? 0) + (metric === "cost" ? bucket.costUsd : processedTokens(bucket)),
    );
  }
  return days.map((day) => {
    const bands = PROVIDER_ORDER.map((provider) => ({
      provider,
      value: values.get(`${day}\0${provider}`) ?? 0,
    }));
    return { day, bands, total: bands.reduce((sum, band) => sum + band.value, 0) };
  });
}

export function niceScale(peak: number, count: number): { max: number; ticks: readonly number[] } {
  if (peak <= 0) return { max: 0, ticks: [0] };
  const rawStep = peak / count;
  const magnitude = 10 ** Math.floor(Math.log10(rawStep));
  const normalized = rawStep / magnitude;
  const step = (normalized > 5 ? 10 : normalized > 2 ? 5 : normalized > 1 ? 2 : 1) * magnitude;
  const max = Math.ceil(peak / step) * step;
  const ticks: number[] = [];
  for (let value = 0; value <= max + step * 1e-6; value += step) ticks.push(value);
  return { max, ticks };
}

function tangents(points: readonly Point[]): readonly number[] {
  if (points.length < 2) return [0];
  const slopes = points.slice(1).map((point, index) => {
    const prior = points[index]!;
    return (point.y - prior.y) / (point.x - prior.x || 1);
  });
  const result = Array.from({ length: points.length }, () => 0);
  result[0] = slopes[0] ?? 0;
  result[result.length - 1] = slopes.at(-1) ?? 0;
  for (let index = 1; index < points.length - 1; index += 1) {
    const previous = slopes[index - 1] ?? 0;
    const next = slopes[index] ?? 0;
    result[index] = previous * next <= 0 ? 0 : (previous + next) / 2;
  }
  for (let index = 0; index < slopes.length; index += 1) {
    const slope = slopes[index] ?? 0;
    if (slope === 0) {
      result[index] = 0;
      result[index + 1] = 0;
      continue;
    }
    const a = (result[index] ?? 0) / slope;
    const b = (result[index + 1] ?? 0) / slope;
    if (a * a + b * b > 9) {
      const scale = 3 / Math.sqrt(a * a + b * b);
      result[index] = scale * a * slope;
      result[index + 1] = scale * b * slope;
    }
  }
  return result;
}

function curvePath(points: readonly Point[]): string {
  if (points.length === 0) return "";
  if (points.length === 1) return `M${points[0]!.x},${points[0]!.y}`;
  const slopes = tangents(points);
  let path = `M${points[0]!.x.toFixed(2)},${points[0]!.y.toFixed(2)}`;
  for (let index = 0; index < points.length - 1; index += 1) {
    const from = points[index]!;
    const to = points[index + 1]!;
    const dx = to.x - from.x;
    path += ` C${(from.x + dx / 3).toFixed(2)},${(from.y + ((slopes[index] ?? 0) * dx) / 3).toFixed(2)} ${(to.x - dx / 3).toFixed(2)},${(to.y - ((slopes[index + 1] ?? 0) * dx) / 3).toFixed(2)} ${to.x.toFixed(2)},${to.y.toFixed(2)}`;
  }
  return path;
}

function formatTokens(value: number): string {
  return new Intl.NumberFormat("en-US", {
    notation: value >= 1_000 ? "compact" : "standard",
    maximumSignificantDigits: 3,
  }).format(value);
}
function formatUsd(value: number): string {
  return new Intl.NumberFormat("en-US", {
    style: "currency",
    currency: "USD",
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  }).format(value);
}
function formatDay(day: string): string {
  const [year, month, date] = day.split("-").map(Number);
  if (!year || !month || !date) return day;
  return new Intl.DateTimeFormat("en-US", {
    month: "short",
    day: "numeric",
    timeZone: "UTC",
  }).format(new Date(Date.UTC(year, month - 1, date)));
}

export function UsageProviderChart({
  sinceDay,
  untilDay,
  buckets,
  metric,
}: UsageProviderChartProps) {
  const days = useMemo(() => enumerateUsageDays(sinceDay, untilDay), [sinceDay, untilDay]);
  const columns = useMemo(() => buildDayColumns(days, buckets, metric), [buckets, days, metric]);
  const [hoverIndex, setHoverIndex] = useState<number | null>(null);
  const plotRef = useRef<HTMLDivElement | null>(null);
  const peak = columns.reduce(
    (max, column) => column.bands.reduce((inner, band) => Math.max(inner, band.value), max),
    0,
  );
  const { max, ticks } = niceScale(peak, 4);
  const stepX = days.length <= 1 ? 0 : VIEW_WIDTH / (days.length - 1);
  const toY = useCallback(
    (value: number) =>
      max === 0 ? VIEW_HEIGHT : VIEW_HEIGHT - (value / max) * (VIEW_HEIGHT - PLOT_TOP),
    [max],
  );
  const paths = PROVIDER_ORDER.map((provider) => {
    const points = columns.map((column, index) => ({
      x: index * stepX,
      y: toY(column.bands.find((band) => band.provider === provider)?.value ?? 0),
    }));
    const line = curvePath(points);
    return {
      provider,
      line,
      area: line === "" ? "" : `${line} L${VIEW_WIDTH},${VIEW_HEIGHT} L0,${VIEW_HEIGHT} Z`,
      total: points.reduce(
        (sum, _, index) =>
          sum + (columns[index]?.bands.find((band) => band.provider === provider)?.value ?? 0),
        0,
      ),
    };
  }).sort((left, right) => right.total - left.total);
  const format = metric === "cost" ? formatUsd : formatTokens;
  const hovered = hoverIndex === null ? undefined : columns[hoverIndex];
  const onMove = useCallback(
    (event: React.MouseEvent<HTMLDivElement>) => {
      const bounds = plotRef.current?.getBoundingClientRect();
      if (!bounds || bounds.width === 0 || days.length === 0) return;
      setHoverIndex(
        Math.min(
          days.length - 1,
          Math.max(
            0,
            Math.round(((event.clientX - bounds.left) / bounds.width) * (days.length - 1)),
          ),
        ),
      );
    },
    [days.length],
  );

  return (
    <div className="flex min-w-0 flex-col gap-1">
      <div className="flex min-w-0 gap-2">
        <div className="relative h-56 w-14 shrink-0" aria-hidden>
          {ticks.map((tick) => (
            <span
              key={tick}
              className="absolute right-0 -translate-y-1/2 text-[10px] text-muted-foreground tabular-nums"
              style={{ top: `${(toY(tick) / VIEW_HEIGHT) * 100}%` }}
            >
              {tick === 0 ? "0" : format(tick)}
            </span>
          ))}
        </div>
        <div
          ref={plotRef}
          className="relative h-56 min-w-0 flex-1 touch-pan-y"
          onMouseMove={onMove}
          onMouseLeave={() => setHoverIndex(null)}
        >
          <svg
            className="h-full w-full"
            viewBox={`0 0 ${VIEW_WIDTH} ${VIEW_HEIGHT}`}
            preserveAspectRatio="none"
            role="img"
            aria-label={`Daily ${metric === "tokens" ? "processed tokens" : "cost"} by provider`}
          >
            {ticks.map((tick) => (
              <line
                key={tick}
                x1={0}
                x2={VIEW_WIDTH}
                y1={toY(tick)}
                y2={toY(tick)}
                stroke="currentColor"
                strokeWidth={1}
                className="text-border"
                vectorEffect="non-scaling-stroke"
              />
            ))}
            {paths.map(({ provider, area }) => (
              <path key={provider} d={area} fill={PROVIDER_COLOR[provider]} fillOpacity={0.12} />
            ))}
            {paths.map(({ provider, line }) => (
              <path
                key={provider}
                d={line}
                fill="none"
                stroke={PROVIDER_COLOR[provider]}
                strokeWidth={2}
                vectorEffect="non-scaling-stroke"
              />
            ))}
            {hoverIndex === null ? null : (
              <line
                x1={hoverIndex * stepX}
                x2={hoverIndex * stepX}
                y1={PLOT_TOP}
                y2={VIEW_HEIGHT}
                stroke="currentColor"
                className="text-muted-foreground"
                vectorEffect="non-scaling-stroke"
              />
            )}
          </svg>
          {hovered ? (
            <div
              role="tooltip"
              className="pointer-events-none absolute top-0 z-10 min-w-36 border border-border bg-background/95 px-2 py-1.5 text-xs"
              style={{
                left: `${days.length <= 1 ? 0 : ((hoverIndex ?? 0) / (days.length - 1)) * 100}%`,
                transform:
                  (hoverIndex ?? 0) / Math.max(1, days.length - 1) > 0.6
                    ? "translateX(-100%)"
                    : undefined,
              }}
            >
              <div className="mb-1 text-muted-foreground">{formatDay(hovered.day)}</div>
              {PROVIDER_ORDER.map((provider) => {
                const Mark = PROVIDER_MARK[provider];
                return (
                  <div key={provider} className="flex items-center justify-between gap-3">
                    <span className="flex items-center gap-1.5 text-muted-foreground">
                      <Mark className="size-3" aria-hidden />
                      {PROVIDER_LABEL[provider]}
                    </span>
                    <span className="tabular-nums">
                      {format(hovered.bands.find((band) => band.provider === provider)?.value ?? 0)}
                    </span>
                  </div>
                );
              })}
              <div className="mt-1 flex justify-between border-t border-border pt-1">
                <span className="text-muted-foreground">Total</span>
                <span className="tabular-nums">{format(hovered.total)}</span>
              </div>
            </div>
          ) : null}
        </div>
      </div>
      <div className="flex justify-between pl-16 text-[10px] text-muted-foreground uppercase">
        <span>{days[0] ? formatDay(days[0]) : ""}</span>
        <span>
          {days[Math.floor(days.length / 2)] ? formatDay(days[Math.floor(days.length / 2)]!) : ""}
        </span>
        <span>{days.at(-1) ? formatDay(days.at(-1)!) : ""}</span>
      </div>
    </div>
  );
}

export function UsageChartLegend() {
  return (
    <div className="flex flex-wrap items-center gap-4">
      {PROVIDER_ORDER.map((provider) => {
        const Mark = PROVIDER_MARK[provider];
        return (
          <span key={provider} className="flex items-center gap-1.5 text-xs text-muted-foreground">
            <Mark className="size-3.5 shrink-0" aria-hidden />
            {PROVIDER_LABEL[provider]}
          </span>
        );
      })}
    </div>
  );
}

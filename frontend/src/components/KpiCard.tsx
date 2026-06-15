// KpiCard — KPI metric card following the .cs-kpi structure from the handoff.
// Wraps in .cs-card; top row = icon box + optional trend badge;
// then label, value (+ unit), trend-base line.

import { Icon } from './Icon'

export interface KpiCardProps {
  icon: string
  iconBg: string
  iconColor: string
  label: string
  value: string | number
  unit?: string
  trend?: string
  trendUp?: boolean
  base?: string
}

export function KpiCard({
  icon,
  iconBg,
  iconColor,
  label,
  value,
  unit,
  trend,
  trendUp,
  base,
}: KpiCardProps) {
  return (
    <div className="cs-card cs-kpi">
      {/* Top row: icon box + trend badge */}
      <div className="cs-kpi-top">
        <span
          className="cs-kpi-ico"
          style={{ background: iconBg, color: iconColor }}
        >
          <Icon name={icon} w={20} />
        </span>
        {trend && (
          <span className={`cs-trend cs-trend--${trendUp !== false ? 'up' : 'down'}`}>
            <Icon name={trendUp !== false ? 'up' : 'down'} w={13} />
            {trend}
          </span>
        )}
      </div>

      {/* Label */}
      <div className="cs-kpi-label">{label}</div>

      {/* Value + unit */}
      <div className="cs-kpi-val">
        {value}
        {unit && <span className="cs-kpi-unit">{unit}</span>}
      </div>

      {/* Trend base */}
      {base && (
        <div style={{ marginTop: 6 }}>
          <span className="cs-trend-base">{base}</span>
        </div>
      )}
    </div>
  )
}

// Form field primitives (Epic 0 foundation): labelled controls with a
// consistent layout and inline error slot, so screens compose forms without
// re-styling every input. Controlled components — callers own the state.

import type { ReactNode } from 'react'

const labelStyle: React.CSSProperties = {
  display: 'block',
  fontSize: 13,
  fontWeight: 500,
  color: 'var(--muted)',
  marginBottom: 4,
}

const controlStyle: React.CSSProperties = {
  width: '100%',
  padding: '9px 14px',
  fontSize: 14,
  boxSizing: 'border-box',
}

export interface FieldProps {
  label?: ReactNode
  error?: string | null
  children: ReactNode
}

export function Field({ label, error, children }: FieldProps) {
  return (
    <div style={{ marginBottom: 'var(--sp-3)' }}>
      {label && <label style={labelStyle}>{label}</label>}
      {children}
      {error && (
        <p role="alert" style={{ color: 'crimson', fontSize: 12, margin: '4px 0 0' }}>
          {error}
        </p>
      )}
    </div>
  )
}

export interface InputProps extends React.InputHTMLAttributes<HTMLInputElement> {
  label?: ReactNode
  error?: string | null
}

export function Input({ label, error, ...rest }: InputProps) {
  return (
    <Field label={label} error={error}>
      <input {...rest} style={{ ...controlStyle, ...rest.style }} />
    </Field>
  )
}

export interface TextareaProps extends React.TextareaHTMLAttributes<HTMLTextAreaElement> {
  label?: ReactNode
  error?: string | null
}

export function Textarea({ label, error, ...rest }: TextareaProps) {
  return (
    <Field label={label} error={error}>
      <textarea {...rest} style={{ ...controlStyle, minHeight: 80, resize: 'vertical', ...rest.style }} />
    </Field>
  )
}

export interface SelectOption {
  value: string | number
  label: string
}

export interface SelectProps extends React.SelectHTMLAttributes<HTMLSelectElement> {
  label?: ReactNode
  error?: string | null
  options: SelectOption[]
  placeholder?: string
}

export function Select({ label, error, options, placeholder, ...rest }: SelectProps) {
  return (
    <Field label={label} error={error}>
      <select {...rest} style={{ ...controlStyle, ...rest.style }}>
        {placeholder && <option value="">{placeholder}</option>}
        {options.map((o) => (
          <option key={o.value} value={o.value}>
            {o.label}
          </option>
        ))}
      </select>
    </Field>
  )
}

export interface ToggleProps {
  label?: ReactNode
  checked: boolean
  onChange: (checked: boolean) => void
  disabled?: boolean
}

export function Toggle({ label, checked, onChange, disabled }: ToggleProps) {
  return (
    <label style={{ display: 'flex', gap: 8, alignItems: 'center', marginBottom: 14, cursor: 'pointer' }}>
      <input
        type="checkbox"
        checked={checked}
        disabled={disabled}
        onChange={(e) => onChange(e.target.checked)}
      />
      {label && <span style={{ fontSize: 14 }}>{label}</span>}
    </label>
  )
}

// File upload control (Epic 0 foundation): drag-and-drop + click-to-pick with
// an upload-in-progress state. Delegates the actual request to the caller via
// onUpload so it works against any backend file endpoint (chat attachments,
// report imports, etc.). Reports progress optimistically by disabling input.

import { useRef, useState } from 'react'

export interface FileUploadProps {
  /// Perform the upload; resolve with an error message or null on success.
  onUpload: (file: File) => Promise<string | null>
  accept?: string
  label?: string
  disabled?: boolean
}

export function FileUpload({ onUpload, accept, label = '拖放或點選上傳檔案', disabled }: FileUploadProps) {
  const inputRef = useRef<HTMLInputElement>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [dragOver, setDragOver] = useState(false)

  const handle = async (file: File | undefined) => {
    if (!file || busy || disabled) return
    setBusy(true)
    setError(null)
    const err = await onUpload(file)
    setBusy(false)
    if (err) setError(err)
    if (inputRef.current) inputRef.current.value = ''
  }

  return (
    <div>
      <div
        onClick={() => !disabled && !busy && inputRef.current?.click()}
        onDragOver={(e) => {
          e.preventDefault()
          setDragOver(true)
        }}
        onDragLeave={() => setDragOver(false)}
        onDrop={(e) => {
          e.preventDefault()
          setDragOver(false)
          void handle(e.dataTransfer.files[0])
        }}
        style={{
          border: `2px dashed ${dragOver ? '#3B82F6' : '#ccc'}`,
          borderRadius: 8,
          padding: 20,
          textAlign: 'center',
          color: '#666',
          cursor: disabled || busy ? 'default' : 'pointer',
          background: dragOver ? '#EFF6FF' : 'transparent',
        }}
      >
        {busy ? '上傳中…' : label}
      </div>
      <input
        ref={inputRef}
        type="file"
        accept={accept}
        disabled={disabled || busy}
        style={{ display: 'none' }}
        onChange={(e) => void handle(e.target.files?.[0])}
      />
      {error && (
        <p role="alert" style={{ color: 'crimson', fontSize: 12, margin: '6px 0 0' }}>
          {error}
        </p>
      )}
    </div>
  )
}

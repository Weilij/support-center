import { Icon } from '../../components/Icon'
import { Drawer } from '../../components/Modal'
import { FileUpload } from '../../components/FileUpload'
import {
  fileDownloadUrl,
  uploadConversationFile,
  type Attachment,
} from '../../stores/files'

export function FilesDrawer({
  open,
  convId,
  files,
  onClose,
  onRefresh,
}: {
  open: boolean
  convId: string | undefined
  files: Attachment[]
  onClose: () => void
  onRefresh: () => Promise<void>
}) {
  return (
    <Drawer
      open={open}
      title="附件檔案"
      onClose={onClose}
      width={420}
    >
      {convId && (
        <>
          <FileUpload
            label="拖放或點選上傳檔案到此對話"
            onUpload={async (file) => {
              const { error } = await uploadConversationFile(convId, file)
              if (!error) await onRefresh()
              return error ?? null
            }}
          />
          <div style={{ marginTop: 12 }}>
            {files.length === 0 ? (
              <p style={{ color: 'var(--muted)', fontSize: 13, margin: 0 }}>尚無檔案</p>
            ) : (
              <ul style={{ listStyle: 'none', padding: 0, margin: 0 }}>
                {files.map((file) => (
                  <li
                    key={file.id}
                    style={{
                      display: 'flex',
                      gap: 8,
                      alignItems: 'center',
                      padding: '6px 0',
                      fontSize: 14,
                      borderBottom: '1px solid var(--line)',
                    }}
                  >
                    <Icon name="paperclip" w={14} style={{ flexShrink: 0, color: 'var(--muted)' }} />
                    <span style={{ flex: 1, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                      {file.originalName || file.filename || file.id}
                    </span>
                    {file.size != null && (
                      <span style={{ color: 'var(--muted)', fontSize: 12, flexShrink: 0 }}>
                        {Math.round(file.size / 1024)} KB
                      </span>
                    )}
                    <button
                      className="cs-btn"
                      style={{ flexShrink: 0, fontSize: 12, padding: '3px 10px' }}
                      onClick={async () => {
                        const url = (await fileDownloadUrl(file.id)) ?? file.publicUrl ?? file.url
                        if (url) window.open(url, '_blank', 'noopener,noreferrer')
                      }}
                    >
                      下載
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </div>
        </>
      )}
    </Drawer>
  )
}

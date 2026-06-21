import { useCallback, useState } from 'react'
import { addTemplate, listTemplates, removeTemplate, updateTemplate, type Template } from '../lib/templates'

export function useTemplates() {
  const [list, setList] = useState<Template[]>(() => listTemplates())
  const refresh = useCallback(() => setList(listTemplates()), [])
  return {
    list,
    add: useCallback((input: { title: string; body: string }) => { addTemplate(input); refresh() }, [refresh]),
    update: useCallback((id: string, patch: Partial<Omit<Template, 'id'>>) => { updateTemplate(id, patch); refresh() }, [refresh]),
    remove: useCallback((id: string) => { removeTemplate(id); refresh() }, [refresh]),
  }
}

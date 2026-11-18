import { extname } from 'node:path'

type ClipboardImageItem = { types: readonly string[]; getType(type: string): Promise<unknown> }

/** Prefer copied pixels; Explorer's copied image files are represented by paths. */
export async function readClipboardImage(deps: {
  readItems(): Promise<ClipboardImageItem[]>
  readFiles(): Promise<string[]>
  imageFromPath(path: string): string | null
}): Promise<string | null> {
  const item = (await deps.readItems()).find(value => value.types.includes('image/png'))
  if (item) {
    const image = await item.getType('image/png') as Blob
    return `data:image/png;base64,${Buffer.from(await image.arrayBuffer()).toString('base64')}`
  }
  for (const path of await deps.readFiles()) {
    if (!['.png', '.jpg', '.jpeg', '.webp', '.bmp', '.gif'].includes(extname(path).toLowerCase())) continue
    const image = deps.imageFromPath(path)
    if (image) return image
  }
  return null
}

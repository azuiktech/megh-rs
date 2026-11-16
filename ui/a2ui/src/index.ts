import { css, html, nothing } from 'lit';
import { z } from 'zod';
import { A2uiLitElement, Catalog, DynamicStringSchema, DynamicValueSchema } from '@a2ui/web_core/v1_0';

/** Where attachments go: the API base, the tenant and the member the signed-in client talks to. */
export type Place = { base: string; tenant: string; member: string };

const FileDataSchema = z.object({ fileUri: z.string(), mimeType: z.string(), displayName: z.string() });
const FileDataListSchema = z.array(FileDataSchema);

/** A file as a model takes it (`fileData`). */
export type FileData = z.infer<typeof FileDataSchema>;

export const AttachmentApi = {
  name: 'Attachment',
  schema: z
    .object({
      label: DynamicStringSchema.optional(),
      value: DynamicValueSchema,
      weight: z.number().optional(),
    })
    .strict(),
};

/** Uploads the files to the place's member; answers their file data. */
export async function upload(place: Place, files: File[]): Promise<FileData[]> {
  const body = new FormData();
  files.forEach(file => body.append('files[]', file));
  const response = await fetch(`${place.base}/tenants/${place.tenant}/members/${place.member}/attachments`, { method: 'POST', body, credentials: 'include' });
  if (!response.ok) throw new Error(`upload refused: ${response.status}`);
  return FileDataListSchema.parse(await response.json());
}

/** The attachment field: browse for files; each is uploaded and shown as a chip until removed. */
export abstract class AttachmentElement extends A2uiLitElement<typeof AttachmentApi> {
  protected abstract readonly place: Place;

  static styles = css`
    :host { display: flex; flex-direction: column; gap: var(--a2ui-spacing-xs, 0.25rem); }
    ul { display: flex; flex-wrap: wrap; gap: var(--a2ui-spacing-s, 0.25rem); margin: 0; padding: 0; list-style: none; }
    li { display: flex; align-items: center; gap: var(--a2ui-spacing-xs, 0.25rem); padding: 2px 8px; border: var(--a2ui-border, 1px solid #ccc); border-radius: 999px; }
    button { border: none; background: none; cursor: pointer; font: inherit; }
  `;

  private files(): FileData[] {
    return FileDataListSchema.safeParse(this.controller.props?.value).data ?? [];
  }

  private keep(files: FileData[]): void {
    this.controller.props?.setValue?.(files);
  }

  private async chosen(event: Event): Promise<void> {
    const input = event.target as HTMLInputElement;
    const added = await upload(this.place, Array.from(input.files ?? []));
    input.value = '';
    this.keep([...this.files(), ...added]);
  }

  render() {
    const props = this.controller.props;
    if (!props) return nothing;
    const chip = (file: FileData) => html`<li>${file.displayName}<button type="button" aria-label="Remove ${file.displayName}" @click=${() => this.keep(this.files().filter(kept => kept.fileUri !== file.fileUri))}>×</button></li>`;
    return html`
      <label>${props.label ?? nothing}<input type="file" multiple @change=${this.chosen}></label>
      <ul>${this.files().map(chip)}</ul>
    `;
  }
}

/** megh's catalog (`azuik.com:megh`), with the attachment field uploading to `place`; register it beside the basic catalog. */
export function meghCatalog(place: Place): Catalog<typeof AttachmentApi & { tagName: string; element: CustomElementConstructor }> {
  const tagName = `megh-attachment-${place.member}`.toLowerCase();
  const element = customElements.get(tagName) ?? placed(tagName, place);
  return new Catalog('azuik.com:megh', '1.0', [{ ...AttachmentApi, tagName, element }]);
}

function placed(tagName: string, place: Place): CustomElementConstructor {
  class Placed extends AttachmentElement {
    protected readonly place = place;
  }
  customElements.define(tagName, Placed);
  return Placed;
}

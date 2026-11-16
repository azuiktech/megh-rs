import { afterEach, describe, expect, it, vi } from 'vitest';
import { MessageProcessor, basicCatalog } from '@a2ui/web_core/v1_0';
import { meghCatalog, upload } from '../src/index';

const place = { base: 'https://app.example.com/api/v1', tenant: 't1', member: 'm1' };

const surface = [
  { version: '1.0', createSurface: { surfaceId: 's1', catalogId: 'https://a2ui.org/specification/v1_0/catalogs/basic/catalog.json', sendDataModel: true } },
  {
    version: '1.0',
    updateComponents: {
      surfaceId: 's1',
      components: [
        { id: 'root', component: 'Column', children: ['c0', 'c1'] },
        { id: 'c0', component: 'Attachment', catalogId: 'azuik.com:megh', label: 'Photos', value: { '@path': '/photos' } },
        { id: 'c1', component: 'Button', child: 'c1t', action: { event: { name: 'submit', context: { photos: { '@path': '/photos' } } } } },
        { id: 'c1t', component: 'Text', text: 'Send' },
      ],
    },
  },
];

afterEach(() => vi.unstubAllGlobals());

describe('meghCatalog', () => {
  it('is the catalog azuik.com:megh, holding Attachment', () => {
    const catalog = meghCatalog(place);
    expect(catalog.id).toBe('azuik.com:megh');
    expect(catalog.components.has('Attachment')).toBe(true);
  });

  it('lets a surface of the basic catalog hold megh Attachment', () => {
    const processor = new MessageProcessor([basicCatalog, meghCatalog(place)], () => {});
    expect(processor.processMessagesWithErrors(surface)).toEqual([]);
    expect(processor.model.surfacesMap.get('s1')?.componentsModel.get('c0')?.type).toBe('Attachment');
  });
});

describe('upload', () => {
  it('posts the files as files[] to the pair and answers their file data', async () => {
    const answered = [{ fileUri: 'https://app.example.com/api/v1/tenants/t1/attachments/a', mimeType: 'image/png', displayName: 'a.png' }];
    const fetch = vi.fn(async () => new Response(JSON.stringify(answered), { status: 200 }));
    vi.stubGlobal('fetch', fetch);

    const files = await upload(place, [new File([new Uint8Array([1, 2])], 'a.png', { type: 'image/png' })]);

    expect(files).toEqual(answered);
    const [url, init] = fetch.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe('https://app.example.com/api/v1/tenants/t1/members/m1/attachments');
    expect(init.method).toBe('POST');
    expect(init.credentials).toBe('include');
    expect((init.body as FormData).getAll('files[]').map(f => (f as File).name)).toEqual(['a.png']);
  });

  it('fails when the server refuses, such as over the size cap', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => new Response('', { status: 413 })));
    await expect(upload(place, [new File(['x'], 'big.bin')])).rejects.toThrow('413');
  });
});

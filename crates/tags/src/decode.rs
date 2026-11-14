use crate::wire::Wire;
use crate::{Error, Result, records::*, wire::*};
use map::{FourCC, Map, RawPointer, ResourceFile, TagId, TagLocation};
use std::io::{Read, Seek};

/// Raw resource location and byte length. Encodings, swizzling and mip packing
/// remain the renderer's responsibility; no GPU-size formula is guessed here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceRange {
    pub pointer: RawPointer,
    pub size: u32,
}
/// Known map lengths, indexed by the raw-pointer file selector.
#[derive(Clone, Debug, Default)]
pub struct ResourceSizes {
    sizes: [Option<u64>; 4],
}
fn selector(file: ResourceFile) -> usize {
    match file {
        ResourceFile::Local => 0,
        ResourceFile::MainMenu => 1,
        ResourceFile::Shared => 2,
        ResourceFile::SinglePlayerShared => 3,
    }
}
impl ResourceSizes {
    pub fn set(&mut self, file: ResourceFile, size: u64) {
        self.sizes[selector(file)] = Some(size);
    }
    pub fn validate(&self, range: ResourceRange) -> Result<()> {
        let p = range.pointer;
        let length = self.sizes[selector(p.file)].ok_or(Error::MissingResource(p.file))?;
        if u64::from(p.offset) + u64::from(range.size) > length {
            return Err(Error::ResourceBounds {
                file: p.file,
                offset: p.offset,
                size: range.size,
                length,
            });
        }
        Ok(())
    }
}

/// Pixel format IDs preserve unknown values for forward inspection. The names
/// identify storage formats, not a promise of implemented texture decoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    A8,
    Y8,
    Ay8,
    A8Y8,
    Rgb565,
    Argb1555,
    Argb4444,
    Xrgb8888,
    Argb8888,
    Dxt1,
    Dxt3,
    Dxt5,
    P8Bump,
    P8,
    ArgbF32,
    RgbF32,
    RgbF16,
    V8U8,
    G8B8,
    Unknown(u16),
}
impl From<u16> for PixelFormat {
    fn from(v: u16) -> Self {
        match v {
            0 => Self::A8,
            1 => Self::Y8,
            2 => Self::Ay8,
            3 => Self::A8Y8,
            6 => Self::Rgb565,
            8 => Self::Argb1555,
            9 => Self::Argb4444,
            10 => Self::Xrgb8888,
            11 => Self::Argb8888,
            14 => Self::Dxt1,
            15 => Self::Dxt3,
            16 => Self::Dxt5,
            17 => Self::P8Bump,
            18 => Self::P8,
            19 => Self::ArgbF32,
            20 => Self::RgbF32,
            21 => Self::RgbF16,
            22 => Self::V8U8,
            23 => Self::G8B8,
            n => Self::Unknown(n),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureKind {
    D2,
    D3,
    Cube,
    Unknown(u16),
}
impl From<u16> for TextureKind {
    fn from(v: u16) -> Self {
        match v {
            0 => Self::D2,
            1 => Self::D3,
            2 => Self::Cube,
            n => Self::Unknown(n),
        }
    }
}
/// Editor/renderer-facing bitmap image, with its wire record retained for edits.
#[derive(Clone)]
pub struct Image {
    pub format: PixelFormat,
    pub kind: TextureKind,
    pub dimensions: [u16; 3],
    /// Additional mip levels as stored, without silently adding a base level.
    pub mip_count: u16,
    pub lods: [Option<ResourceRange>; 3],
    /// Low-detail data is local to the header's low-detail bitmap region.
    pub low_detail: Option<ResourceRange>,
    pub wire: BitmapImage,
}
impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Image")
            .field("format", &Compact(&self.format))
            .field("kind", &Compact(&self.kind))
            .field("dimensions", &Compact(&self.dimensions))
            .field("mip_count", &self.mip_count)
            .field("lods", &Compact(&self.lods))
            .field("low_detail", &Compact(&self.low_detail))
            .field("wire", &self.wire)
            .finish()
    }
}
impl Image {
    pub fn is_swizzled(&self) -> bool {
        self.wire.flags.0 & 8 != 0
    }
    pub fn is_linear(&self) -> bool {
        self.wire.flags.0 & 16 != 0
    }
}
#[derive(Clone, Debug)]
pub struct Sequence {
    pub wire: BitmapSequence,
    pub sprites: Vec<BitmapSprite>,
}
#[derive(Clone, Debug)]
pub struct Bitmap {
    pub header: BitmapHeader,
    pub images: Vec<Image>,
    pub sequences: Vec<Sequence>,
}
/// Header-level domain objects retain reviewed fields and opaque bytes. Only
/// bitmap sequences/images/sprites have reviewed child record definitions.
#[derive(Clone, Debug)]
pub struct Scenario {
    pub header: ScenarioHeader,
}
#[derive(Clone, Debug)]
pub struct Globals {
    pub header: GlobalsHeader,
    pub locales: Vec<LocaleDescriptor>,
}
#[derive(Clone, Debug)]
pub struct StructureBsp {
    pub header: BspHeader,
}
#[derive(Clone, Debug)]
pub struct RenderModel {
    pub header: RenderModelHeader,
    pub name: Option<String>,
}

/// Strict rejects observed-empty slots that become populated. Lenient exposes
/// them as warnings without dereferencing pointers with unknown element sizes.
/// Neither mode changes bounds checks for reviewed fields or supports new cache builds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DecodeMode {
    #[default]
    Strict,
    Lenient,
}
/// An unsupported block was preserved, not validated or decoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodeWarning {
    pub record: &'static str,
    pub field: &'static str,
    pub block: TagBlock,
}

/// Counts of the actual structural checks performed, including empty fields.
#[derive(Clone, Copy, Debug, Default)]
pub struct Validation {
    pub records: usize,
    pub blocks: usize,
    pub nonempty_blocks: usize,
    pub block_elements: u64,
    pub data_references: usize,
    pub tag_references: usize,
    pub string_ids: usize,
}
#[derive(Clone, Debug)]
pub enum TagData {
    Bitmap(Box<Bitmap>),
    Scenario(Box<Scenario>),
    Globals(Box<Globals>),
    Bsp(Box<StructureBsp>),
    RenderModel(Box<RenderModel>),
}
#[derive(Clone, Debug)]
pub struct DecodedTag {
    pub id: TagId,
    pub group: FourCC,
    pub data: TagData,
    pub validation: Validation,
    pub warnings: Vec<DecodeWarning>,
}

impl DecodedTag {
    pub fn supports(group: FourCC) -> bool {
        matches!(&group.0, b"bitm" | b"scnr" | b"matg" | b"sbsp" | b"mode")
    }
    /// Read a supported tag and validate reviewed ranges, bitmap children,
    /// root references and string IDs. Opaque spans are preserved, not scanned.
    /// Raw ranges are exposed; use `validate_resources` with all file lengths.
    pub fn read<R: Read + Seek>(map: &mut Map<R>, id: TagId) -> Result<Self> {
        Self::read_with_mode(map, id, DecodeMode::Strict)
    }
    /// Read with an explicit unsupported-block policy. Lenient warnings do not
    /// count as successful range checks; opaque bytes are never pointer-scanned.
    pub fn read_with_mode<R: Read + Seek>(
        map: &mut Map<R>,
        id: TagId,
        mode: DecodeMode,
    ) -> Result<Self> {
        let tag = map.tag(id)?.clone();
        if !Self::supports(tag.group) {
            return Err(Error::UnsupportedGroup(tag.group));
        }
        let location = tag.location.ok_or(map::Error::NoTagData)?;
        let bytes = map.read_tag_bytes(id)?;
        let payload = Payload {
            bytes: &bytes,
            location,
            mode,
            warnings: std::cell::RefCell::new(Vec::new()),
        };
        let mut validation = Validation::default();
        let mut root_refs = Vec::new();
        let mut root_strings = Vec::new();
        let data = match &tag.group.0 {
            b"bitm" => {
                let header: BitmapHeader = payload.record(0, &mut validation)?;
                let image_bytes = payload.block(header.images, BitmapImage::SIZE as u32)?;
                let mut images = Vec::new();
                for b in image_bytes.as_chunks::<{ BitmapImage::SIZE }>().0 {
                    let wire: BitmapImage = payload.nested(b, &mut validation)?;
                    if wire.signature != FourCC(*b"bitm").as_u32() {
                        return Err(Error::Invalid("bitmap image signature"));
                    }
                    let mut lods = [None; 3];
                    for (i, lod) in lods.iter_mut().enumerate() {
                        if wire.lod_sizes[i] == 0 {
                            continue;
                        }
                        let pointer = RawPointer::decode(wire.lod_pointers[i])
                            .ok_or(Error::Invalid("nonempty bitmap LOD has null pointer"))?;
                        *lod = Some(ResourceRange {
                            pointer,
                            size: wire.lod_sizes[i],
                        });
                    }
                    let low_detail = if wire.low_detail_size == 0 {
                        None
                    } else {
                        let h = map.header();
                        if u64::from(wire.low_detail_offset) + u64::from(wire.low_detail_size)
                            > u64::from(h.low_detail_size)
                        {
                            return Err(Error::Invalid("low-detail bitmap range"));
                        }
                        let offset = h
                            .low_detail_offset
                            .checked_add(wire.low_detail_offset)
                            .ok_or(Error::Invalid("low-detail bitmap offset overflow"))?;
                        if u64::from(offset) + u64::from(wire.low_detail_size)
                            > u64::from(h.file_size)
                        {
                            return Err(Error::Invalid("low-detail data outside map"));
                        }
                        Some(ResourceRange {
                            pointer: RawPointer {
                                file: ResourceFile::Local,
                                offset,
                            },
                            size: wire.low_detail_size,
                        })
                    };
                    images.push(Image {
                        format: wire.format.0.into(),
                        kind: wire.kind.0.into(),
                        dimensions: [wire.width, wire.height, u16::from(wire.depth)],
                        mip_count: wire.mip_count,
                        lods,
                        low_detail,
                        wire,
                    });
                }
                let mut sequences = Vec::new();
                // Aliased sprite spans must not amplify allocation quadratically.
                // This budget bounds the total decoded records, not just each span.
                let sprite_budget = bytes.len() as u64 / BitmapSprite::SIZE as u64;
                let mut decoded_sprites = 0u64;
                for b in payload
                    .block(header.sequences, BitmapSequence::SIZE as u32)?
                    .as_chunks::<{ BitmapSequence::SIZE }>()
                    .0
                {
                    let wire: BitmapSequence = payload.nested(b, &mut validation)?;
                    decoded_sprites += u64::from(wire.sprites.count);
                    if decoded_sprites > sprite_budget {
                        return Err(Error::Invalid("total sprite count exceeds payload budget"));
                    }
                    let mut sprites = Vec::new();
                    for s in payload
                        .block(wire.sprites, BitmapSprite::SIZE as u32)?
                        .as_chunks::<{ BitmapSprite::SIZE }>()
                        .0
                    {
                        sprites.push(payload.nested(s, &mut validation)?);
                    }
                    sequences.push(Sequence { wire, sprites });
                }
                TagData::Bitmap(Box::new(Bitmap {
                    header,
                    images,
                    sequences,
                }))
            }
            b"scnr" => {
                let header: ScenarioHeader = payload.record(0, &mut validation)?;
                collect_references(
                    ScenarioHeader::META,
                    &bytes,
                    &mut root_refs,
                    &mut root_strings,
                )?;
                TagData::Scenario(Box::new(Scenario { header }))
            }
            b"matg" => {
                let header: GlobalsHeader = payload.record(0, &mut validation)?;
                collect_references(
                    GlobalsHeader::META,
                    &bytes,
                    &mut root_refs,
                    &mut root_strings,
                )?;
                let mut locales = Vec::new();
                for i in 0..9 {
                    let locale: LocaleDescriptor =
                        payload.record(0x188 + i * LocaleDescriptor::SIZE, &mut validation)?;
                    let limit = u64::from(map.header().file_size);
                    if (locale.count != 0
                        && u64::from(locale.index_file_offset) + u64::from(locale.count) * 8
                            > limit)
                        || (locale.data_size != 0
                            && u64::from(locale.data_file_offset) + u64::from(locale.data_size)
                                > limit)
                    {
                        return Err(Error::Invalid("locale file range"));
                    }
                    locales.push(locale);
                }
                TagData::Globals(Box::new(Globals { header, locales }))
            }
            b"sbsp" => {
                let header: BspHeader = payload.record(0, &mut validation)?;
                collect_references(BspHeader::META, &bytes, &mut root_refs, &mut root_strings)?;
                TagData::Bsp(Box::new(StructureBsp { header }))
            }
            b"mode" => {
                let header: RenderModelHeader = payload.record(0, &mut validation)?;
                collect_references(
                    RenderModelHeader::META,
                    &bytes,
                    &mut root_refs,
                    &mut root_strings,
                )?;
                let name = header.name.resolve(map.strings())?.map(str::to_owned);
                TagData::RenderModel(Box::new(RenderModel { header, name }))
            }
            _ => unreachable!(),
        };
        for reference in root_refs {
            validation.tag_references += 1;
            if reference.id == TagId::NULL {
                continue;
            }
            let target = map.tag(reference.id)?;
            let group = map.groups().iter().find(|g| g.class == target.group);
            if reference.group != target.group
                && !group.is_some_and(|g| {
                    g.parent == Some(reference.group) || g.grandparent == Some(reference.group)
                })
            {
                return Err(Error::Invalid("tag reference class mismatch"));
            }
        }
        for string in root_strings {
            validation.string_ids += 1;
            string.resolve(map.strings())?;
        }
        Ok(Self {
            id,
            group: tag.group,
            data,
            validation,
            warnings: payload.warnings.into_inner(),
        })
    }
    /// Validate all exposed bitmap raw ranges against the selected files.
    /// Missing file lengths are errors, never counted as verified.
    pub fn validate_resources(&self, sizes: &ResourceSizes) -> Result<usize> {
        let mut count = 0;
        if let TagData::Bitmap(bitmap) = &self.data {
            for image in &bitmap.images {
                for range in image.lods.iter().flatten().chain(image.low_detail.iter()) {
                    sizes.validate(*range)?;
                    count += 1;
                }
            }
        }
        Ok(count)
    }
}

pub(crate) struct Payload<'a> {
    pub bytes: &'a [u8],
    pub location: TagLocation,
    pub mode: DecodeMode,
    pub warnings: std::cell::RefCell<Vec<DecodeWarning>>,
}
impl<'a> Payload<'a> {
    pub(crate) fn range(&self, address: u32, size: u64) -> Result<&'a [u8]> {
        // Empty fields may carry stale pointers. No dereference is performed.
        if size == 0 {
            return Ok(&[]);
        }
        let size32 = u32::try_from(size).map_err(|_| Error::Bounds { address, size })?;
        self.location.space.translate(address, size32)?;
        let start = address
            .checked_sub(self.location.address)
            .ok_or(Error::Bounds { address, size })? as usize;
        let end = start
            .checked_add(size32 as usize)
            .ok_or(Error::Bounds { address, size })?;
        self.bytes
            .get(start..end)
            .ok_or(Error::Bounds { address, size })
    }
    pub(crate) fn block(&self, b: TagBlock, stride: u32) -> Result<&'a [u8]> {
        if b.count != 0 && !b.address.is_multiple_of(4) {
            return Err(Error::Invalid("unaligned reviewed block pointer"));
        }
        self.range(b.address, u64::from(b.count) * u64::from(stride))
    }
    pub(crate) fn record<T: BinaryRecord>(
        &self,
        offset: usize,
        stats: &mut Validation,
    ) -> Result<T> {
        let b = self
            .bytes
            .get(
                offset
                    ..offset
                        .checked_add(T::SIZE)
                        .ok_or(Error::Invalid("record offset overflow"))?,
            )
            .ok_or(Error::RecordSize {
                actual: self.bytes.len().saturating_sub(offset),
                expected: T::SIZE,
            })?;
        self.nested(b, stats)
    }
    fn nested<T: BinaryRecord>(&self, b: &[u8], stats: &mut Validation) -> Result<T> {
        let value = T::decode(b)?;
        self.validate_record(T::META, b, stats)?;
        stats.records += 1;
        Ok(value)
    }
    fn validate_record(&self, meta: RecordMeta, b: &[u8], stats: &mut Validation) -> Result<()> {
        for field in meta.fields {
            let bytes = &b[field.offset..field.offset + field.size];
            let result = (|| -> Result<()> {
                match field.kind {
                    FieldKind::Block(_) | FieldKind::EmptyBlock => {
                        let block = TagBlock::read(bytes)?;
                        stats.blocks += 1;
                        if block.count != 0 {
                            if field.kind == FieldKind::EmptyBlock {
                                if self.mode == DecodeMode::Strict {
                                    return Err(Error::UnsupportedBlock(field.name));
                                }
                                self.warnings.borrow_mut().push(DecodeWarning {
                                    record: meta.name,
                                    field: field.name,
                                    block,
                                });
                                return Ok(());
                            }
                            let FieldKind::Block(stride) = field.kind else {
                                unreachable!()
                            };
                            self.block(block, stride)?;
                            stats.nonempty_blocks += 1;
                            stats.block_elements += u64::from(block.count);
                        }
                    }
                    FieldKind::DataRef => {
                        let r = DataReference::read(bytes)?;
                        self.range(r.address, u64::from(r.size))?;
                        stats.data_references += 1;
                    }
                    _ => {}
                }
                Ok(())
            })();
            result.map_err(|source| Error::Field {
                record: meta.name,
                field: field.name,
                source: Box::new(source),
            })?;
        }
        Ok(())
    }
}
fn collect_references(
    meta: RecordMeta,
    b: &[u8],
    refs: &mut Vec<TagReference>,
    strings: &mut Vec<StringId>,
) -> Result<()> {
    for f in meta.fields {
        let bytes = &b[f.offset..f.offset + f.size];
        match f.kind {
            FieldKind::TagRef => refs.push(TagReference::read(bytes)?),
            FieldKind::StringId => strings.push(StringId::read(bytes)?),
            _ => {}
        }
    }
    Ok(())
}

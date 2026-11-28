//! Windows: printing a message with Windows' own print dialog and print
//! system, with no other app (a PDF viewer) in between.
//!
//! 1. Windows' print dialog (`PrintDlgExW`: the printer, its Preferences,
//!    copies, pages) shows on a thread of its own, owned by the window, as
//!    GPUI shows its file dialogs: the window waits for it, and its thread
//!    keeps drawing. The settings chosen are kept for the next print.
//! 2. The message is laid out on the paper the printer has
//!    (`print::message_pdf_on`), as a vector PDF in memory, never on disk.
//! 3. Windows' own PDF engine (Windows.Data.Pdf) reads it and draws each
//!    page with Direct2D into a print job (`ID2D1PrintControl`, Windows'
//!    XPS print path), as vectors: text stays text, with its fonts. Copies,
//!    collation, two-sided printing and the rest go with the job's print
//!    ticket, made from the dialog's settings.
//! 4. Should that fail before the job is complete, it is cancelled and the
//!    pages go to the same printer, with the same settings, as pictures
//!    through GDI, at the printer's resolution up to 300 dpi.
use anyhow::{Context as _, Result};
use std::sync::Mutex;
use windows::Data::Pdf::{PdfDocument, PdfPage, PdfPageRenderOptions};
use windows::Storage::Streams::IRandomAccessStream;
use windows::Win32::Foundation::{GlobalFree, HGLOBAL, HWND};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_COLOR_F, D2D_RECT_F, D2D_SIZE_F,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_COLOR_SPACE_SRGB, D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_FACTORY_TYPE_SINGLE_THREADED,
    D2D1_PRINT_CONTROL_PROPERTIES, D2D1_PRINT_FONT_SUBSET_MODE_DEFAULT, D2D1CreateFactory,
    ID2D1DeviceContext, ID2D1Factory1,
};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice, ID3D11Device,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateDCW, CreateICW, DEVMODEA, DEVMODEW, DIB_RGB_COLORS, DeleteDC,
    GetDeviceCaps, HALFTONE, HDC, LOGPIXELSX, LOGPIXELSY, PHYSICALHEIGHT, PHYSICALOFFSETX, PHYSICALOFFSETY,
    PHYSICALWIDTH, SRCCOPY, SetStretchBltMode, StretchDIBits,
};
use windows::Win32::Graphics::Imaging::{CLSID_WICImagingFactory, IWICImagingFactory};
use windows::Win32::Graphics::Printing::PrintTicket::{
    PTCloseProvider, PTConvertDevModeToPrintTicket, PTOpenProvider, kPTJobScope,
};
use windows::Win32::Storage::Xps::Printing::{
    IPrintDocumentPackageTargetFactory, PrintDocumentPackageTargetFactory,
};
use windows::Win32::Storage::Xps::{AbortDoc, DOCINFOW, EndDoc, EndPage, StartDocW, StartPage};
use windows::Win32::System::Com::StructuredStorage::{CreateStreamOnHGlobal, GetHGlobalFromStream};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, COINIT_MULTITHREADED,
    CoCreateInstance, CoInitializeEx, CoUninitialize, IStream, STREAM_SEEK_SET,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::WinRT::Pdf::{IPdfRendererNative, PDF_RENDER_PARAMS};
use windows::Win32::System::WinRT::{BSOS_DEFAULT, CreateRandomAccessStreamOverStream};
use windows::Win32::UI::Controls::Dialogs::{
    DEVNAMES, PD_HIDEPRINTTOFILE, PD_NOCURRENTPAGE, PD_NOSELECTION, PD_PAGENUMS, PD_RESULT_PRINT,
    PD_USEDEVMODECOPIESANDCOLLATE, PRINTDLGEXW, PRINTPAGERANGE, PrintDlgExW, START_PAGE_GENERAL,
};
use windows::core::{IUnknown, Interface as _, PCWSTR, w};

/// The most page ranges the dialog takes ("1-2, 5, 7-9").
const MAX_RANGES: usize = 16;
/// The highest page number the dialog takes (`print::MAX_PAGES`).
const MAX_PAGE: u32 = 200;
/// The highest resolution pages go to a printer at as pictures (GDI).
const RASTER_DPI: i32 = 300;
/// The resolution Direct2D gives parts it has to make pictures of.
const PRINT_RASTER_DPI: f32 = 300.;
/// Direct2D's unit: 1/96 in.
const DIPS_PER_INCH: f32 = 96.;

/// The printer settings chosen last (its DEVMODE and DEVNAMES), which the
/// dialog starts from next time.
static LAST: Mutex<Option<(Vec<u8>, Vec<u8>)>> = Mutex::new(None);

/// COM on this thread, for as long as it's kept.
struct Apartment(bool);

impl Apartment {
    /// A single-threaded apartment, which the print dialog needs.
    fn single() -> Self {
        // SAFETY: paired with CoUninitialize in Drop when it succeeded.
        Self(unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) }.is_ok())
    }
    /// The multithreaded apartment, for printing: Windows.Data.Pdf's
    /// operations are waited for here, which a single-threaded apartment
    /// shouldn't.
    fn multi() -> Self {
        // SAFETY: as above.
        Self(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok())
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: CoInitializeEx succeeded on this thread.
            unsafe { CoUninitialize() };
        }
    }
}

/// A DEVMODEW kept aligned for the APIs that take a pointer to one.
struct DevMode {
    words: Vec<u64>,
}

impl DevMode {
    /// `bytes` when they hold a whole DEVMODEW, its driver's part included.
    fn new(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < std::mem::size_of::<DEVMODEW>() {
            return None;
        }
        let mut words = vec![0u64; bytes.len().div_ceil(8)];
        // SAFETY: `words` holds at least `bytes.len()` bytes.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), words.as_mut_ptr().cast::<u8>(), bytes.len()) };
        let this = Self { words };
        let dm = this.as_ptr();
        // SAFETY: at least a DEVMODEW's size, aligned.
        let whole = unsafe { (*dm).dmSize as usize + (*dm).dmDriverExtra as usize };
        (whole <= bytes.len() && whole >= std::mem::offset_of!(DEVMODEW, dmFields)).then_some(this)
    }
    fn as_ptr(&self) -> *const DEVMODEW {
        self.words.as_ptr().cast()
    }
    /// Its size, the driver's part included.
    fn size(&self) -> u32 {
        // SAFETY: checked in `new`.
        unsafe { (*self.as_ptr()).dmSize as u32 + (*self.as_ptr()).dmDriverExtra as u32 }
    }
}

/// What the print dialog chose.
pub struct Choice {
    /// The printer's name, ending in NUL.
    printer: Vec<u16>,
    devmode: DevMode,
    /// The pages asked for, numbered from 1, each `(first, last)`; empty for all.
    ranges: Vec<(u32, u32)>,
    /// The printer's paper, (width, height) in PDF points, as it will be
    /// printed on (turned for landscape).
    paper: Option<(f32, f32)>,
}

impl Choice {
    /// The printer's paper in PDF points, where the printer says.
    pub fn paper(&self) -> Option<(f32, f32)> {
        self.paper
    }
    /// The printer's name.
    pub fn printer(&self) -> String {
        let end = self.printer.iter().position(|&c| c == 0).unwrap_or(self.printer.len());
        String::from_utf16_lossy(&self.printer[..end])
    }
    fn printer_ptr(&self) -> PCWSTR {
        PCWSTR(self.printer.as_ptr())
    }
    /// Whether page `index` (from 0) of `count` is to be printed.
    fn prints(&self, index: u32) -> bool {
        let page = index + 1;
        self.ranges.is_empty() || self.ranges.iter().any(|&(a, b)| (a.min(b)..=a.max(b)).contains(&page))
    }
}

/// The bytes of a movable global memory block.
///
/// # Safety
/// `h` is null or a global memory handle.
unsafe fn global_bytes(h: HGLOBAL) -> Option<Vec<u8>> {
    if h.0.is_null() {
        return None;
    }
    // SAFETY: a valid handle, locked while read.
    unsafe {
        let p = GlobalLock(h);
        if p.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(p.cast::<u8>(), GlobalSize(h)).to_vec();
        let _ = GlobalUnlock(h);
        Some(bytes)
    }
}

/// A new movable global memory block holding `bytes` (the dialog's to free).
fn global_from(bytes: &[u8]) -> Option<HGLOBAL> {
    // SAFETY: a new block of `bytes.len()` bytes, locked while written.
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len()).ok()?;
        let p = GlobalLock(h);
        if p.is_null() {
            let _ = GlobalFree(Some(h));
            return None;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p.cast::<u8>(), bytes.len());
        let _ = GlobalUnlock(h);
        Some(h)
    }
}

/// The printer named in a DEVNAMES block, ending in NUL.
fn printer_in(devnames: &[u8]) -> Option<Vec<u16>> {
    if devnames.len() < std::mem::size_of::<DEVNAMES>() {
        return None;
    }
    // SAFETY: at least a DEVNAMES' size; read unaligned.
    let names: DEVNAMES = unsafe { std::ptr::read_unaligned(devnames.as_ptr().cast()) };
    let start = names.wDeviceOffset as usize * 2;
    let chars: Vec<u16> = devnames
        .get(start..)?
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|&c| c != 0)
        .collect();
    if chars.is_empty() {
        return None;
    }
    Some(chars.into_iter().chain([0]).collect())
}

/// Shows Windows' print dialog, owned by window `owner`. `None` when it
/// was closed without printing.
fn choose(owner: isize) -> Result<Option<Choice>> {
    let mut ranges = [PRINTPAGERANGE::default(); MAX_RANGES];
    // SAFETY: PRINTDLGEXW is plain data (its callback field is a null
    // interface pointer when zeroed); the handles given to the dialog are
    // its to replace, and every handle it returns is freed below.
    unsafe {
        let mut pd: PRINTDLGEXW = std::mem::zeroed();
        pd.lStructSize = std::mem::size_of::<PRINTDLGEXW>() as u32;
        pd.hwndOwner = HWND(owner as *mut _);
        let last = LAST.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some((devmode, devnames)) = last {
            pd.hDevMode = global_from(&devmode).unwrap_or_default();
            pd.hDevNames = global_from(&devnames).unwrap_or_default();
        }
        pd.Flags = PD_NOSELECTION | PD_NOCURRENTPAGE | PD_USEDEVMODECOPIESANDCOLLATE | PD_HIDEPRINTTOFILE;
        pd.nMaxPageRanges = MAX_RANGES as u32;
        pd.lpPageRanges = ranges.as_mut_ptr();
        pd.nMinPage = 1;
        pd.nMaxPage = MAX_PAGE;
        pd.nCopies = 1;
        pd.nStartPage = START_PAGE_GENERAL;
        let shown = PrintDlgExW(&mut pd);
        let devmode = global_bytes(pd.hDevMode);
        let devnames = global_bytes(pd.hDevNames);
        for h in [pd.hDevMode, pd.hDevNames] {
            if !h.0.is_null() {
                let _ = GlobalFree(Some(h));
            }
        }
        shown.context(t!("html-print-windows-dialog-failed"))?;
        // Kept for next time, printed or only applied.
        if let (Some(devmode), Some(devnames)) = (&devmode, &devnames) {
            *LAST.lock().unwrap_or_else(|e| e.into_inner()) = Some((devmode.clone(), devnames.clone()));
        }
        if pd.dwResultAction != PD_RESULT_PRINT {
            return Ok(None);
        }
        let printer = devnames
            .as_deref()
            .and_then(printer_in)
            .ok_or_else(|| anyhow::anyhow!(t!("html-print-windows-no-printer")))?;
        let devmode = devmode
            .as_deref()
            .and_then(DevMode::new)
            .ok_or_else(|| anyhow::anyhow!(t!("html-print-windows-no-printer")))?;
        let ranges = if (pd.Flags & PD_PAGENUMS).0 != 0 {
            ranges[..(pd.nPageRanges as usize).min(MAX_RANGES)]
                .iter()
                .map(|r| (r.nFromPage, r.nToPage))
                .collect()
        } else {
            Vec::new()
        };
        let mut choice = Choice { printer, devmode, ranges, paper: None };
        choice.paper = paper(&choice);
        Ok(Some(choice))
    }
}

/// The paper the printer prints on with these settings, in PDF points.
fn paper(choice: &Choice) -> Option<(f32, f32)> {
    // SAFETY: an information context for the printer, released below.
    unsafe {
        let ic = CreateICW(w!("WINSPOOL"), choice.printer_ptr(), PCWSTR::null(), Some(choice.devmode.as_ptr()));
        if ic.0.is_null() {
            return None;
        }
        let caps = |i| GetDeviceCaps(Some(ic), i);
        let (width, height, dpi_x, dpi_y) = (caps(PHYSICALWIDTH), caps(PHYSICALHEIGHT), caps(LOGPIXELSX), caps(LOGPIXELSY));
        let _ = DeleteDC(ic);
        (width > 0 && height > 0 && dpi_x > 0 && dpi_y > 0)
            .then(|| (width as f32 / dpi_x as f32 * 72., height as f32 / dpi_y as f32 * 72.))
    }
}

/// `pdf` as Windows.Data.Pdf's document, read from memory.
fn document(pdf: &[u8]) -> Result<PdfDocument> {
    // SAFETY: a stream on new global memory, written whole then read.
    unsafe {
        let stream = CreateStreamOnHGlobal(HGLOBAL::default(), true)?;
        let len = u32::try_from(pdf.len()).context(t!("html-print-windows-too-large"))?;
        stream.Write(pdf.as_ptr().cast(), len, None).ok()?;
        stream.Seek(0, STREAM_SEEK_SET, None)?;
        let random: IRandomAccessStream = CreateRandomAccessStreamOverStream(&stream, BSOS_DEFAULT)?;
        Ok(PdfDocument::LoadFromStreamAsync(&random)?.join()?)
    }
}

/// The job's print ticket, from the dialog's settings (paper, copies,
/// collation, two-sided…).
fn ticket(choice: &Choice) -> Result<IStream> {
    // SAFETY: a print ticket provider for the printer, closed below; the
    // DEVMODE is whole (`DevMode::new`).
    unsafe {
        let provider = PTOpenProvider(choice.printer_ptr(), 1)?;
        let stream = CreateStreamOnHGlobal(HGLOBAL::default(), true);
        let converted = stream.as_ref().map_err(Clone::clone).and_then(|stream| {
            PTConvertDevModeToPrintTicket(
                provider,
                choice.devmode.size(),
                choice.devmode.as_ptr().cast::<DEVMODEA>(),
                kPTJobScope,
                stream,
            )
        });
        let _ = PTCloseProvider(provider);
        converted?;
        let stream = stream?;
        stream.Seek(0, STREAM_SEEK_SET, None)?;
        Ok(stream)
    }
}

/// Windows.Data.Pdf's Direct2D renderer for `device`.
///
/// Its `PdfCreateRenderer` is looked up when first printing, from System32
/// alone, rather than imported: imported, a Windows without
/// windows.data.pdf.dll (Server Core, Wine) couldn't start Picacho at all.
fn pdf_renderer(device: &IDXGIDevice) -> Result<IPdfRendererNative> {
    type Create = unsafe extern "system" fn(*mut std::ffi::c_void, *mut *mut std::ffi::c_void) -> windows::core::HRESULT;
    static LIBRARY: std::sync::OnceLock<Option<libloading::Library>> = std::sync::OnceLock::new();
    let library = LIBRARY
        .get_or_init(|| {
            // SAFETY: a system library, whose initialization has no
            // requirements of ours.
            unsafe {
                libloading::os::windows::Library::load_with_flags(
                    "windows.data.pdf.dll",
                    libloading::os::windows::LOAD_LIBRARY_SEARCH_SYSTEM32,
                )
                .ok()
                .map(libloading::Library::from)
            }
        })
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!(t!("html-print-windows-no-pdf-engine")))?;
    // SAFETY: `PdfCreateRenderer` has this signature
    // (<windows.data.pdf.interop.h>); its result is an owned reference.
    unsafe {
        let create = library
            .get::<Create>(b"PdfCreateRenderer\0")
            .map_err(|_| anyhow::anyhow!(t!("html-print-windows-no-pdf-engine")))?;
        let mut renderer = std::ptr::null_mut();
        create(device.as_raw(), &mut renderer).ok()?;
        if renderer.is_null() {
            anyhow::bail!(t!("html-print-windows-no-pdf-engine"));
        }
        Ok(IPdfRendererNative::from_raw(renderer))
    }
}

/// A Direct3D device for Direct2D: the graphics card's, else WARP
/// (software), which every Windows has.
fn d3d_device() -> Result<ID3D11Device> {
    let make = |driver: D3D_DRIVER_TYPE| -> windows::core::Result<ID3D11Device> {
        let mut device = None;
        // SAFETY: creates a device into `device`.
        unsafe {
            D3D11CreateDevice(
                None,
                driver,
                Default::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                None,
            )?;
        }
        device.ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL))
    };
    Ok(make(D3D_DRIVER_TYPE_HARDWARE).or_else(|_| make(D3D_DRIVER_TYPE_WARP))?)
}

/// What became of Direct2D's printing.
enum Vector {
    Printed,
    /// It failed with the job cancelled (nothing printed): GDI may print.
    Cancelled(anyhow::Error),
    /// It failed as the job was being completed: some of it may print.
    Failed(anyhow::Error),
}

/// Prints the pages chosen of `doc` as vectors (Direct2D's XPS printing).
fn print_vector(choice: &Choice, doc: &PdfDocument, title: &str) -> Vector {
    let job = Wide::new(title);
    let started = (|| -> Result<_> {
        let device = d3d_device()?;
        let dxgi: IDXGIDevice = device.cast()?;
        // SAFETY: COM and Direct2D objects created and used on this thread.
        unsafe {
            let factory: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let d2d = factory.CreateDevice(&dxgi)?;
            let context: ID2D1DeviceContext = d2d.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
            let renderer = pdf_renderer(&dxgi)?;
            let ticket = ticket(choice)?;
            let targets: IPrintDocumentPackageTargetFactory =
                CoCreateInstance(&PrintDocumentPackageTargetFactory, None, CLSCTX_INPROC_SERVER)?;
            let target =
                targets.CreateDocumentPackageTargetForPrintJob(choice.printer_ptr(), job.as_pcwstr(), None, &ticket)?;
            let properties = D2D1_PRINT_CONTROL_PROPERTIES {
                fontSubset: D2D1_PRINT_FONT_SUBSET_MODE_DEFAULT,
                rasterDPI: PRINT_RASTER_DPI,
                colorSpace: D2D1_COLOR_SPACE_SRGB,
            };
            let control = match d2d.CreatePrintControl(&wic, &target, Some(&properties)) {
                Ok(control) => control,
                Err(e) => {
                    let _ = target.Cancel();
                    return Err(e.into());
                }
            };
            Ok((context, renderer, target, control))
        }
    })();
    let (context, renderer, target, control) = match started {
        Ok(parts) => parts,
        Err(e) => return Vector::Cancelled(e),
    };
    let pages = (|| -> Result<()> {
        for index in 0..doc.PageCount()? {
            if !choice.prints(index) {
                continue;
            }
            let page = doc.GetPage(index)?;
            let size = page.Size()?;
            let params = PDF_RENDER_PARAMS {
                SourceRect: D2D_RECT_F::default(),
                DestinationWidth: size.Width.round().max(1.) as u32,
                DestinationHeight: size.Height.round().max(1.) as u32,
                BackgroundColor: D2D_COLOR_F { r: 1., g: 1., b: 1., a: 1. },
                IgnoreHighContrast: true,
            };
            let unknown: IUnknown = page.cast()?;
            // SAFETY: Direct2D drawing into a command list on this thread,
            // between BeginDraw and EndDraw.
            unsafe {
                let list = context.CreateCommandList()?;
                context.SetTarget(&list);
                context.BeginDraw();
                let drawn = renderer.RenderPageToDeviceContext(&unknown, &context, Some(&params));
                let ended = context.EndDraw(None, None);
                context.SetTarget(None::<&windows::Win32::Graphics::Direct2D::ID2D1Image>);
                drawn?;
                ended?;
                list.Close()?;
                control.AddPage(&list, D2D_SIZE_F { width: size.Width, height: size.Height }, None, None, None)?;
            }
        }
        Ok(())
    })();
    if let Err(e) = pages {
        // SAFETY: cancels the job begun above; nothing of it prints.
        unsafe {
            let _ = target.Cancel();
        }
        return Vector::Cancelled(e);
    }
    // SAFETY: completes the job (everything goes to the spooler).
    match unsafe { control.Close() } {
        Ok(()) => Vector::Printed,
        Err(e) => {
            // SAFETY: as above, for whatever wasn't sent.
            unsafe {
                let _ = target.Cancel();
            }
            Vector::Failed(e.into())
        }
    }
}

/// Text as a NUL-terminated wide string (a job's name).
struct Wide(Vec<u16>);

impl Wide {
    fn new(text: &str) -> Self {
        Self(text.encode_utf16().filter(|&c| c != 0).chain([0]).collect())
    }
    fn as_pcwstr(&self) -> PCWSTR {
        PCWSTR(self.0.as_ptr())
    }
}

/// One page of `doc` as a top-down BGRA picture `width` pixels across.
fn page_picture(page: &PdfPage, width: u32, height: u32) -> Result<(Vec<u8>, u32, u32)> {
    let options = PdfPageRenderOptions::new()?;
    options.SetDestinationWidth(width)?;
    options.SetDestinationHeight(height)?;
    options.SetBackgroundColor(windows::UI::Color { A: 255, R: 255, G: 255, B: 255 })?;
    options.SetIsIgnoringHighContrast(true)?;
    // SAFETY: a stream on new global memory, written by the renderer and
    // read back whole.
    let png = unsafe {
        let stream = CreateStreamOnHGlobal(HGLOBAL::default(), true)?;
        let random: IRandomAccessStream = CreateRandomAccessStreamOverStream(&stream, BSOS_DEFAULT)?;
        page.RenderWithOptionsToStreamAsync(&random, &options)?.join()?;
        let memory = GetHGlobalFromStream(&stream)?;
        let mut size = 0u64;
        stream.Seek(0, windows::Win32::System::Com::STREAM_SEEK_END, Some(&mut size))?;
        let bytes = global_bytes(memory).unwrap_or_default();
        bytes[..(size as usize).min(bytes.len())].to_vec()
    };
    let picture = image::load_from_memory(&png)?.to_rgba8();
    let (w, h) = picture.dimensions();
    let mut bgra = picture.into_raw();
    for px in bgra.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    Ok((bgra, w, h))
}

/// Prints the pages chosen of `doc` as pictures through GDI.
fn print_raster(choice: &Choice, doc: &PdfDocument, title: &str) -> Result<()> {
    let name = Wide::new(title);
    // SAFETY: a device context for the printer, its document begun and
    // ended (or aborted) here, then released.
    unsafe {
        let dc: HDC = CreateDCW(w!("WINSPOOL"), choice.printer_ptr(), PCWSTR::null(), Some(choice.devmode.as_ptr()));
        if dc.0.is_null() {
            anyhow::bail!(t!("html-print-windows-printer-unavailable", printer = choice.printer()));
        }
        let info = DOCINFOW {
            cbSize: std::mem::size_of::<DOCINFOW>() as i32,
            lpszDocName: name.as_pcwstr(),
            ..Default::default()
        };
        if StartDocW(dc, &info) <= 0 {
            let _ = DeleteDC(dc);
            anyhow::bail!(t!("html-print-windows-printer-unavailable", printer = choice.printer()));
        }
        let caps = |i| GetDeviceCaps(Some(dc), i);
        let (dpi_x, dpi_y) = (caps(LOGPIXELSX).max(1), caps(LOGPIXELSY).max(1));
        let (offset_x, offset_y) = (caps(PHYSICALOFFSETX), caps(PHYSICALOFFSETY));
        let dpi = dpi_x.min(dpi_y).min(RASTER_DPI) as f32;
        SetStretchBltMode(dc, HALFTONE);
        let printed = (|| -> Result<()> {
            for index in 0..doc.PageCount()? {
                if !choice.prints(index) {
                    continue;
                }
                let page = doc.GetPage(index)?;
                let size = page.Size()?;
                let inches = (size.Width / DIPS_PER_INCH, size.Height / DIPS_PER_INCH);
                let (bits, w, h) = page_picture(
                    &page,
                    (inches.0 * dpi).round().max(1.) as u32,
                    (inches.1 * dpi).round().max(1.) as u32,
                )?;
                let header = BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w as i32,
                    biHeight: -(h as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                };
                let bitmap = BITMAPINFO { bmiHeader: header, ..Default::default() };
                if StartPage(dc) <= 0 {
                    anyhow::bail!(t!("html-print-windows-printer-unavailable", printer = choice.printer()));
                }
                // The page from the paper's corner: the device's origin is
                // its printable area's.
                let drawn = StretchDIBits(
                    dc,
                    -offset_x,
                    -offset_y,
                    (inches.0 * dpi_x as f32).round() as i32,
                    (inches.1 * dpi_y as f32).round() as i32,
                    0,
                    0,
                    w as i32,
                    h as i32,
                    Some(bits.as_ptr().cast()),
                    &bitmap,
                    DIB_RGB_COLORS,
                    SRCCOPY,
                );
                if drawn == 0 {
                    anyhow::bail!(t!("html-print-windows-printer-unavailable", printer = choice.printer()));
                }
                if EndPage(dc) <= 0 {
                    anyhow::bail!(t!("html-print-windows-printer-unavailable", printer = choice.printer()));
                }
            }
            Ok(())
        })();
        match printed {
            Ok(()) => {
                let ended = EndDoc(dc);
                let _ = DeleteDC(dc);
                if ended <= 0 {
                    anyhow::bail!(t!("html-print-windows-printer-unavailable", printer = choice.printer()));
                }
                Ok(())
            }
            Err(e) => {
                AbortDoc(dc);
                let _ = DeleteDC(dc);
                Err(e)
            }
        }
    }
}

/// Prints a message: Windows' print dialog, owned by window `owner`, then
/// `pdf` (which lays the message out on the paper given, in PDF points)
/// printed to the printer chosen. `chosen` is called once the dialog has
/// closed with Print, before the message is laid out. Returns the
/// printer's name, or `None` when the dialog was closed without printing.
///
/// Waits for the dialog and for printing: for a thread of its own.
pub fn print_message(
    owner: isize,
    title: &str,
    pdf: impl FnOnce(Option<(f32, f32)>) -> Result<Vec<u8>>,
    chosen: impl FnOnce(),
) -> Result<Option<String>> {
    let choice = {
        let _apartment = Apartment::single();
        choose(owner)?
    };
    let Some(choice) = choice else {
        return Ok(None);
    };
    chosen();
    let pdf = pdf(choice.paper())?;
    let _apartment = Apartment::multi();
    let doc = document(&pdf).context(t!("html-print-windows-couldnt-read"))?;
    let count = doc.PageCount()?;
    if !(0..count).any(|i| choice.prints(i)) {
        anyhow::bail!(t!("html-print-windows-no-such-pages", count = count));
    }
    let title = if title.trim().is_empty() { t!("html-print-message") } else { title.trim() };
    match print_vector(&choice, &doc, title) {
        Vector::Printed => {}
        Vector::Failed(e) => return Err(e.context(t!("html-print-windows-couldnt-print", printer = choice.printer()))),
        Vector::Cancelled(e) => {
            eprintln!("Picacho: Direct2D printing failed ({e:#}); printing pages as pictures");
            print_raster(&choice, &doc, title)
                .with_context(|| t!("html-print-windows-couldnt-print", printer = choice.printer()))?;
        }
    }
    Ok(Some(choice.printer()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printer_names_and_page_ranges() {
        // DEVNAMES: offsets in characters, then "drv\0Printer One\0port\0".
        let mut bytes = Vec::new();
        for offset in [4u16, 8, 20, 0] {
            bytes.extend(offset.to_le_bytes());
        }
        for text in ["drv", "Printer One", "port"] {
            bytes.extend(text.encode_utf16().chain([0]).flat_map(u16::to_le_bytes));
        }
        let printer = printer_in(&bytes).unwrap();
        assert_eq!(String::from_utf16_lossy(&printer[..printer.len() - 1]), "Printer One");
        assert_eq!(printer.last(), Some(&0));
        assert_eq!(printer_in(&bytes[..4]), None);
        let mut dm = vec![0u8; std::mem::size_of::<DEVMODEW>()];
        let size = std::mem::size_of::<DEVMODEW>() as u16;
        dm[std::mem::offset_of!(DEVMODEW, dmSize)..][..2].copy_from_slice(&size.to_le_bytes());
        let choice = Choice {
            printer,
            devmode: DevMode::new(&dm).unwrap(),
            ranges: vec![(1, 2), (5, 5)],
            paper: None,
        };
        assert_eq!(choice.printer(), "Printer One");
        assert_eq!((0..6).map(|i| choice.prints(i)).collect::<Vec<_>>(), [true, true, false, false, true, false]);
        assert_eq!(choice.devmode.size() as usize, dm.len());
        assert!(DevMode::new(&dm[..10]).is_none());
    }
}

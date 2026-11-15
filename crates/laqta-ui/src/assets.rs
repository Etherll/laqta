//! The icon asset source: gpui-component's default icons plus the Lucide icons the views name.
//! SVGs are embedded and borrowed from static storage; nothing is read from disk.

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};
pub(crate) use gpui_kit_assets::IconName;

gpui_kit_assets::icon_assets!(
    ViewIcons,
    [
        // The titlebar controls (`chrome.rs`); embedded here so they never depend on the kit's file-backed bundle.
        WindowClose,
        ALargeSmall,
        AppWindow,
        ArrowDown,
        ArrowRight,
        ArrowUp,
        Bell,
        Blend,
        Braces,
        Camera,
        Check,
        ChevronDown,
        ChevronLeft,
        ChevronRight,
        Circle,
        CircleCheck,
        CircleDot,
        CircleStop,
        CircleX,
        Clipboard,
        CloudUpload,
        Copy,
        Cpu,
        Crop,
        Database,
        Droplet,
        Ellipsis,
        Eraser,
        ExternalLink,
        Eye,
        EyeOff,
        FaceSlightlySmiling,
        File,
        Film,
        Flashlight,
        FlipHorizontal2,
        FlipVertical2,
        Folder,
        FolderOpen,
        Grid2x2,
        Grid3x3,
        GripVertical,
        Highlighter,
        House,
        Image,
        ImagePlus,
        Images,
        Info,
        Keyboard,
        Layers,
        LayoutGrid,
        Link,
        List,
        ListChecks,
        ListOrdered,
        LoaderCircle,
        Lock,
        Maximize,
        MessageSquare,
        Mic,
        Minus,
        Monitor,
        MousePointer,
        MousePointer2,
        MousePointerClick,
        MoveUpRight,
        PaintBucket,
        Pause,
        PenLine,
        Pin,
        Pipette,
        Plug,
        Plus,
        QrCode,
        Redo2,
        RotateCcw,
        RotateCw,
        Save,
        Scan,
        ScanLine,
        ScanText,
        Scissors,
        Search,
        Settings,
        Settings2,
        Slash,
        SlidersHorizontal,
        Sparkles,
        Square,
        SquareDashed,
        SquareDashedMousePointer,
        SquarePen,
        Stamp,
        Tag,
        Timer,
        Trash,
        TriangleAlert,
        Type,
        Undo2,
        Upload,
        Video,
        WandSparkles,
        WindowMaximize,
        WindowMinimize,
        WindowRestore,
        Wrench,
        X,
        ZoomIn,
        ZoomOut,
    ]
);

/// The app icon, for the titlebar wordmark. 64 px, drawn at 18 so it stays sharp at 2× and up.
pub(crate) const BRAND_MARK: &str = "brand/laqta-64.png";

pub(crate) struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path == BRAND_MARK {
            return Ok(Some(Cow::Borrowed(include_bytes!("../../../assets/icon/laqta-64.png"))));
        }
        match ViewIcons.load(path)? {
            Some(bytes) => Ok(Some(bytes)),
            None => gpui_kit_assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut all = ViewIcons.list(path)?;
        all.extend(gpui_kit_assets::Assets.list(path)?);
        Ok(all)
    }
}

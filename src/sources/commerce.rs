//! Drives App Store updates through Apple's private CommerceKit framework,
//! the same way mas does. Every call checks that the class and method exist
//! first, and Objective-C exceptions become errors instead of crashes, so a
//! macOS release that changes the framework makes the update fail cleanly.
//!
//! Since macOS 26.1, CommerceKit downloads the update but its installer step
//! fails with error 201. Like mas, update-all keeps hard links to the
//! downloaded package and receipt, then installs them with `installer`.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::fs;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use block2::RcBlock;
use objc2::rc::{Allocated, Retained, autoreleasepool};
use objc2::runtime::{AnyClass, AnyObject, Bool, NSObject, NSObjectProtocol, Sel};
use objc2::{AnyThread, DefinedClass, define_class, msg_send, sel};
use objc2_foundation::{NSDate, NSDefaultRunLoopMode, NSRunLoop, NSString};

use crate::copy::{self, fill};
use crate::exec;
use crate::http;

unsafe extern "C" {
    fn dlopen(path: *const c_char, mode: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

const RTLD_LAZY: c_int = 1;
const COMMERCE_KIT: &CStr = c"/System/Library/PrivateFrameworks/CommerceKit.framework/CommerceKit";
const STORE_FOUNDATION: &CStr =
    c"/System/Library/PrivateFrameworks/StoreFoundation.framework/StoreFoundation";
/// The installer step that fails on macOS 26.1 and later.
const INSTALL_ERROR: isize = 201;

#[derive(Default)]
struct Progress {
    /// Set when the download leaves the queue.
    finished: bool,
    error: Option<String>,
    error_code: Option<isize>,
    /// Set when the purchase request itself fails.
    rejected: Option<String>,
    percent: f32,
    last_change: Option<Instant>,
}

struct Ivars {
    adam_id: u64,
    progress: Arc<Mutex<Progress>>,
    /// Hard links to the package and receipt, kept for the error 201 path.
    links: PathBuf,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and this class
    // doesn't implement Drop.
    #[unsafe(super(NSObject))]
    #[name = "UpdateAllDownloadObserver"]
    #[ivars = Ivars]
    struct Observer;

    impl Observer {
        #[unsafe(method(downloadQueue:changedWithAddition:))]
        fn added(&self, _queue: Option<&AnyObject>, _download: Option<&AnyObject>) {}

        #[unsafe(method(downloadQueue:statusChangedForDownload:))]
        fn status_changed(&self, _queue: Option<&AnyObject>, download: Option<&AnyObject>) {
            let Some(download) = download else { return };
            if item_id(download) != Some(self.ivars().adam_id) {
                return;
            }
            self.keep_links();
            let percent = status(download)
                .filter(|s| responds(s, sel!(percentComplete)))
                .map(|s| unsafe { msg_send![&*s, percentComplete] })
                .unwrap_or(0.0f32);
            if let Ok(mut p) = self.ivars().progress.lock()
                && (percent - p.percent).abs() >= 0.01
            {
                p.percent = percent;
                p.last_change = Some(Instant::now());
            }
        }

        #[unsafe(method(downloadQueue:changedWithRemoval:))]
        fn removed(&self, _queue: Option<&AnyObject>, download: Option<&AnyObject>) {
            let Some(download) = download else { return };
            if item_id(download) != Some(self.ivars().adam_id) {
                return;
            }
            let (error, code) = status(download).map(|s| failure(&s)).unwrap_or((None, None));
            if let Ok(mut p) = self.ivars().progress.lock() {
                p.finished = true;
                p.error = error;
                p.error_code = code;
            }
        }
    }

    unsafe impl NSObjectProtocol for Observer {}
);

impl Observer {
    fn new(ivars: Ivars) -> Retained<Self> {
        let this = Self::alloc().set_ivars(ivars);
        unsafe { msg_send![super(this), init] }
    }

    /// Hard-links the newest package and the receipt out of CommerceKit's
    /// download folder, which it deletes when its own install fails.
    fn keep_links(&self) {
        let Some(dir) = download_dir(self.ivars().adam_id) else {
            return;
        };
        let links = &self.ivars().links;
        let _ = fs::create_dir_all(links);
        let newest_pkg = fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "pkg"))
            .max_by_key(|p| fs::metadata(p).and_then(|m| m.modified()).ok());
        if let Some(pkg) = newest_pkg {
            let target = links.join("update.pkg");
            let _ = fs::remove_file(&target);
            let _ = fs::hard_link(pkg, target);
        }
        let receipt = dir.join("receipt");
        if receipt.exists() {
            let target = links.join("receipt");
            let _ = fs::remove_file(&target);
            let _ = fs::hard_link(receipt, target);
        }
    }
}

fn responds(obj: &AnyObject, sel: Sel) -> bool {
    unsafe { msg_send![obj, respondsToSelector: sel] }
}

fn item_id(download: &AnyObject) -> Option<u64> {
    if !responds(download, sel!(metadata)) {
        return None;
    }
    let metadata: Option<Retained<AnyObject>> = unsafe { msg_send![download, metadata] };
    let metadata = metadata?;
    responds(&metadata, sel!(itemIdentifier))
        .then(|| unsafe { msg_send![&*metadata, itemIdentifier] })
}

fn status(download: &AnyObject) -> Option<Retained<AnyObject>> {
    if !responds(download, sel!(status)) {
        return None;
    }
    unsafe { msg_send![download, status] }
}

/// Reads why a download stopped, if it failed or someone cancelled it.
fn failure(status: &AnyObject) -> (Option<String>, Option<isize>) {
    let error: Option<Retained<AnyObject>> = if responds(status, sel!(error)) {
        unsafe { msg_send![status, error] }
    } else {
        None
    };
    if let Some(err) = error {
        let code: isize = unsafe { msg_send![&*err, code] };
        let text: Retained<NSString> = unsafe { msg_send![&*err, localizedDescription] };
        return (Some(text.to_string()), Some(code));
    }
    if responds(status, sel!(isFailed)) && unsafe { msg_send![status, isFailed] } {
        return (Some(copy::ERR_STORE_FAILED.to_string()), None);
    }
    if responds(status, sel!(isCancelled)) && unsafe { msg_send![status, isCancelled] } {
        return (Some(copy::ERR_STORE_CANCELLED.to_string()), None);
    }
    (None, None)
}

/// CommerceKit's download folder for one item.
fn download_dir(adam_id: u64) -> Option<PathBuf> {
    let handle = unsafe { dlopen(COMMERCE_KIT.as_ptr(), RTLD_LAZY) };
    if handle.is_null() {
        return None;
    }
    let sym = unsafe { dlsym(handle, c"CKDownloadDirectory".as_ptr()) };
    if sym.is_null() {
        return None;
    }
    let f: extern "C" fn(*mut AnyObject) -> *mut AnyObject = unsafe { std::mem::transmute(sym) };
    autoreleasepool(|_| {
        let ptr = f(std::ptr::null_mut());
        if ptr.is_null() {
            return None;
        }
        let s: &NSString = unsafe { &*(ptr as *const NSString) };
        Some(PathBuf::from(s.to_string()).join(adam_id.to_string()))
    })
}

fn class(name: &CStr) -> Result<&'static AnyClass> {
    AnyClass::get(name)
        .ok_or_else(|| anyhow!(fill(copy::ERR_STORE_MISSING, &[&name.to_string_lossy()])))
}

/// Asks the App Store to download and install the newest version of an app,
/// then waits for it to finish.
pub fn update(adam_id: u64, app: &Path) -> Result<()> {
    unsafe {
        if dlopen(COMMERCE_KIT.as_ptr(), RTLD_LAZY).is_null()
            || dlopen(STORE_FOUNDATION.as_ptr(), RTLD_LAZY).is_null()
        {
            bail!(copy::ERR_STORE_UNAVAILABLE);
        }
    }
    let links = http::cache_dir().join("appstore").join(adam_id.to_string());
    let _ = fs::remove_dir_all(&links);
    let progress = Arc::new(Mutex::new(Progress::default()));
    let started = objc2::exception::catch(AssertUnwindSafe(|| start(adam_id, &progress, &links)))
        .map_err(|e| anyhow!(fill(copy::ERR_STORE_EXCEPTION, &[&format!("{e:?}")])))??;
    let result = wait(&progress);
    let _ = objc2::exception::catch(AssertUnwindSafe(|| unsafe {
        match &started.token {
            Some(token) => {
                let _: () = msg_send![&*started.queue, removeObserver: &**token];
            }
            None => {
                let _: () = msg_send![&*started.queue, removeObserver: &*started.observer];
            }
        }
    }));
    drop(started);
    let outcome = match result {
        Err(code) if code == Some(INSTALL_ERROR) => install_from_links(&links, app),
        Err(_) => {
            let p = progress.lock().map_err(|_| anyhow!(copy::ERR_STORE_LOST))?;
            bail!(
                "{}",
                p.rejected
                    .clone()
                    .or_else(|| p.error.clone())
                    .unwrap_or_else(|| copy::ERR_STORE_UNFINISHED.to_string())
            )
        }
        Ok(()) => Ok(()),
    };
    let _ = fs::remove_dir_all(&links);
    outcome
}

/// Objects that must stay alive until the download finishes.
struct Started {
    queue: Retained<AnyObject>,
    token: Option<Retained<AnyObject>>,
    observer: Retained<Observer>,
    _done: RcBlock<dyn Fn(*mut AnyObject, Bool, *mut AnyObject, *mut AnyObject)>,
}

fn start(adam_id: u64, progress: &Arc<Mutex<Progress>>, links: &Path) -> Result<Started> {
    let queue_class = class(c"CKDownloadQueue")?;
    let purchase_class = class(c"SSPurchase")?;
    let metadata_class = class(c"SSDownloadMetadata")?;
    let controller_class = class(c"CKPurchaseController")?;

    let observer = Observer::new(Ivars {
        adam_id,
        progress: progress.clone(),
        links: links.to_path_buf(),
    });
    let queue: Option<Retained<AnyObject>> = unsafe { msg_send![queue_class, sharedDownloadQueue] };
    let queue = queue.ok_or_else(|| anyhow!(copy::ERR_STORE_QUEUE))?;
    let token: Option<Retained<AnyObject>> = unsafe { msg_send![&*queue, addObserver: &*observer] };

    let params = NSString::from_str(&format!(
        "productType=C&price=0&pg=default&appExtVrsId=0&pricingParameters=STDRDL&salableAdamId={adam_id}"
    ));
    let purchase: Option<Retained<AnyObject>> =
        unsafe { msg_send![purchase_class, purchaseWithBuyParameters: &*params] };
    let purchase = purchase.ok_or_else(|| anyhow!(copy::ERR_STORE_REQUEST))?;
    unsafe {
        let _: () = msg_send![&*purchase, setIsRedownload: Bool::YES];
        let _: () = msg_send![&*purchase, setIsUpdate: Bool::YES];
        let _: () = msg_send![&*purchase, setItemIdentifier: adam_id];
    }
    let kind = NSString::from_str("software");
    let alloc: Allocated<AnyObject> = unsafe { msg_send![metadata_class, alloc] };
    let metadata: Option<Retained<AnyObject>> = unsafe { msg_send![alloc, initWithKind: &*kind] };
    let metadata = metadata.ok_or_else(|| anyhow!(copy::ERR_STORE_REQUEST))?;
    unsafe {
        let _: () = msg_send![&*metadata, setItemIdentifier: adam_id];
        let _: () = msg_send![&*purchase, setDownloadMetadata: &*metadata];
    }

    let controller: Option<Retained<AnyObject>> =
        unsafe { msg_send![controller_class, sharedPurchaseController] };
    let controller = controller.ok_or_else(|| anyhow!(copy::ERR_STORE_PURCHASE))?;
    let state = progress.clone();
    let done = RcBlock::new(
        move |_purchase: *mut AnyObject,
              _ok: Bool,
              error: *mut AnyObject,
              response: *mut AnyObject| {
            let mut problem = None;
            if let Some(err) = unsafe { error.as_ref() } {
                let text: Retained<NSString> = unsafe { msg_send![err, localizedDescription] };
                problem = Some(text.to_string());
            } else if let Some(resp) = unsafe { response.as_ref() } {
                let downloads: Option<Retained<AnyObject>> = if responds(resp, sel!(downloads)) {
                    unsafe { msg_send![resp, downloads] }
                } else {
                    None
                };
                let count: usize = downloads
                    .map(|d| unsafe { msg_send![&*d, count] })
                    .unwrap_or(0);
                if count == 0 {
                    problem = Some(copy::ERR_STORE_NOTHING.to_string());
                }
            }
            if let Ok(mut p) = state.lock() {
                p.last_change = Some(Instant::now());
                if problem.is_some() {
                    p.rejected = problem;
                    p.finished = true;
                }
            }
        },
    );
    unsafe {
        let _: () = msg_send![
            &*controller,
            performPurchase: &*purchase,
            withOptions: 0u64,
            completionHandler: &*done
        ];
    }
    Ok(Started {
        queue,
        token,
        observer,
        _done: done,
    })
}

/// Spins the main run loop, where CommerceKit delivers its callbacks, until
/// the download leaves the queue. Gives up after 10 minutes without progress.
fn wait(progress: &Arc<Mutex<Progress>>) -> std::result::Result<(), Option<isize>> {
    let run_loop = NSRunLoop::currentRunLoop();
    let started = Instant::now();
    let mut shown = -1i32;
    loop {
        let step = NSDate::dateWithTimeIntervalSinceNow(0.25);
        unsafe {
            run_loop.runMode_beforeDate(NSDefaultRunLoopMode, &step);
        }
        let Ok(p) = progress.lock() else {
            return Err(None);
        };
        let pct = (p.percent * 100.0) as i32;
        if pct != shown && pct > 0 {
            shown = pct;
            print!("\r  {pct}%");
            let _ = std::io::Write::flush(&mut std::io::stdout());
        }
        if p.finished {
            println!();
            return match (&p.rejected, &p.error) {
                (None, None) => Ok(()),
                _ => Err(p.error_code),
            };
        }
        let idle = p.last_change.map_or(started.elapsed(), |t| t.elapsed());
        if idle > Duration::from_secs(600) {
            println!();
            return Err(None);
        }
    }
}

/// The error 201 path: installs the kept package with `installer`, then puts
/// the receipt back so Spotlight and the App Store still see the app.
fn install_from_links(links: &Path, app: &Path) -> Result<()> {
    let pkg = links.join("update.pkg");
    let receipt = links.join("receipt");
    if !pkg.exists() {
        bail!(copy::ERR_STORE_GONE);
    }
    if !exec::sudo_start() {
        bail!(copy::ERR_NO_SUDO);
    }
    let pkg_s = pkg.display().to_string();
    if !exec::run_live(
        "/usr/sbin/installer",
        &["-pkg".into(), pkg_s, "-target".into(), "/".into()],
        true,
    ) {
        bail!(copy::ERR_STORE_INSTALLER);
    }
    if receipt.exists() {
        let dir = app.join("Contents/_MASReceipt");
        let script =
            r#"mkdir -pm 755 "$1" && cp -cf "$2" "$3" && chown 0:0 "$3" && chmod 644 "$3""#;
        exec::run_live(
            "/bin/sh",
            &[
                "-c".into(),
                script.into(),
                "--".into(),
                dir.display().to_string(),
                receipt.display().to_string(),
                dir.join("receipt").display().to_string(),
            ],
            true,
        );
    }
    let app_s = app.display().to_string();
    let _ = exec::run("mdimport", &[&app_s], Duration::from_secs(60));
    let _ = exec::run(
        "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister",
        &["-f", &app_s],
        Duration::from_secs(30),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Checks, without starting any download, that this macOS still has every
    /// class and method the App Store updater calls.
    #[test]
    fn commerce_kit_has_expected_api() {
        unsafe {
            assert!(!dlopen(COMMERCE_KIT.as_ptr(), RTLD_LAZY).is_null());
            assert!(!dlopen(STORE_FOUNDATION.as_ptr(), RTLD_LAZY).is_null());
        }
        let class_methods = [
            (c"CKDownloadQueue", sel!(sharedDownloadQueue)),
            (c"CKPurchaseController", sel!(sharedPurchaseController)),
            (c"SSPurchase", sel!(purchaseWithBuyParameters:)),
        ];
        for (name, sel) in class_methods {
            let cls = class(name).unwrap();
            assert!(cls.class_method(sel).is_some(), "{name:?} lacks +{sel:?}");
        }
        let instance_methods = [
            (c"CKDownloadQueue", sel!(addObserver:)),
            (c"CKDownloadQueue", sel!(removeObserver:)),
            (
                c"CKPurchaseController",
                sel!(performPurchase:withOptions:completionHandler:),
            ),
            (c"SSPurchase", sel!(setIsRedownload:)),
            (c"SSPurchase", sel!(setIsUpdate:)),
            (c"SSPurchase", sel!(setItemIdentifier:)),
            (c"SSPurchase", sel!(setDownloadMetadata:)),
            (c"SSDownloadMetadata", sel!(initWithKind:)),
            (c"SSDownloadMetadata", sel!(setItemIdentifier:)),
        ];
        for (name, sel) in instance_methods {
            let cls = class(name).unwrap();
            assert!(
                cls.instance_method(sel).is_some(),
                "{name:?} lacks -{sel:?}"
            );
        }
        assert!(download_dir(1).is_some());
    }
}

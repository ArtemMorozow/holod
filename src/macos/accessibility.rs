use std::ptr::{self, NonNull};

use objc2_application_services::{
    AXCopyMultipleAttributeOptions, AXError, AXIsProcessTrusted, AXUIElement, AXValue, AXValueType,
};
use objc2_core_foundation::{CFArray, CFBoolean, CFRetained, CFString, CFType, CGPoint, CGSize};

use crate::core::window::WindowError;

const AX_TIMEOUT_SECONDS: f32 = 0.25;

pub fn require_access() -> Result<(), WindowError> {
    // SAFETY: The documented trust check takes no arguments and has no preconditions.
    if unsafe { AXIsProcessTrusted() } {
        Ok(())
    } else {
        Err(WindowError::AccessibilityDenied)
    }
}

pub fn configure_timeout() -> Result<(), WindowError> {
    let system = system_wide();
    // SAFETY: The system-wide AX object is live and the timeout is positive.
    let result = unsafe { system.set_messaging_timeout(AX_TIMEOUT_SECONDS) };
    check(result, "set messaging timeout")
}

pub fn system_wide() -> CFRetained<AXUIElement> {
    // SAFETY: The initializer takes no arguments and returns an owned AX object.
    unsafe { AXUIElement::new_system_wide() }
}

pub fn pid(element: &AXUIElement) -> Result<i32, AXError> {
    let mut pid = 0;
    // SAFETY: The AX object is live and the out pointer is writable.
    let result = unsafe { element.pid(NonNull::from(&mut pid)) };
    if result == AXError::Success {
        Ok(pid)
    } else {
        Err(result)
    }
}

pub fn application(pid: i32) -> CFRetained<AXUIElement> {
    debug_assert!(pid > 0);
    // SAFETY: The caller supplies the positive PID of a running user application.
    unsafe { AXUIElement::new_application(pid) }
}

pub fn read(element: &AXUIElement, attribute: &'static str) -> Result<CFRetained<CFType>, AXError> {
    let name = CFString::from_static_str(attribute);
    let mut value = ptr::null();
    // SAFETY: Both input objects are retained for the call, and the out pointer is writable.
    let result = unsafe { element.copy_attribute_value(&name, NonNull::from(&mut value)) };
    if result != AXError::Success {
        return Err(result);
    }
    let value = NonNull::new(value.cast_mut()).ok_or(AXError::NoValue)?;
    // SAFETY: A successful Copy call returns an owned CF object at the checked pointer.
    Ok(unsafe { CFRetained::from_raw(value) })
}

pub fn read_many(
    element: &AXUIElement,
    attributes: &[&CFString],
) -> Result<CFRetained<CFArray<CFType>>, AXError> {
    let attributes = CFArray::from_objects(attributes);
    let mut values = ptr::null();
    // SAFETY: The input array holds CFStrings, and the out pointer is writable.
    let result = unsafe {
        element.copy_multiple_attribute_values(
            attributes.as_opaque(),
            AXCopyMultipleAttributeOptions(0),
            NonNull::from(&mut values),
        )
    };
    if result != AXError::Success {
        return Err(result);
    }
    let values = NonNull::new(values.cast_mut()).ok_or(AXError::NoValue)?;
    // SAFETY: A successful Copy call returns an owned array of CF objects.
    Ok(unsafe { CFRetained::from_raw(values.cast()) })
}

pub fn windows(element: &AXUIElement) -> Result<CFRetained<CFArray<CFType>>, AXError> {
    let array = read(element, "AXWindows")?
        .downcast::<CFArray>()
        .map_err(|_| AXError::Failure)?;
    // SAFETY: AXWindows is an array of CF objects. The caller checks each element's type.
    Ok(unsafe { CFRetained::cast_unchecked(array) })
}

pub fn point(value: &AXValue) -> Option<CGPoint> {
    let mut point = CGPoint::default();
    // SAFETY: AXValue checks the requested type before writing into the CGPoint buffer.
    unsafe { value.value(AXValueType::CGPoint, NonNull::from(&mut point).cast()) }.then_some(point)
}

pub fn size(value: &AXValue) -> Option<CGSize> {
    let mut size = CGSize::default();
    // SAFETY: AXValue checks the requested type before writing into the CGSize buffer.
    unsafe { value.value(AXValueType::CGSize, NonNull::from(&mut size).cast()) }.then_some(size)
}

pub fn read_bool(element: &AXUIElement, attribute: &'static str) -> Result<bool, AXError> {
    read(element, attribute)?
        .downcast::<CFBoolean>()
        .map(|value| value.as_bool())
        .map_err(|_| AXError::Failure)
}

pub fn set(element: &AXUIElement, attribute: &'static str, value: &CFType) -> AXError {
    let name = CFString::from_static_str(attribute);
    // SAFETY: The element, attribute and CF value remain live throughout the call.
    unsafe { element.set_attribute_value(&name, value) }
}

pub fn raise(element: &AXUIElement) -> AXError {
    let action = CFString::from_static_str("AXRaise");
    // SAFETY: The element and action remain live throughout the call.
    unsafe { element.perform_action(&action) }
}

pub fn check(result: AXError, operation: &'static str) -> Result<(), WindowError> {
    match result {
        AXError::Success => Ok(()),
        AXError::APIDisabled => Err(WindowError::AccessibilityDenied),
        AXError::InvalidUIElement => Err(WindowError::WindowUnavailable),
        code => Err(WindowError::Accessibility {
            operation,
            code: code.0,
        }),
    }
}

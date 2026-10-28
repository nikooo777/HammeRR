macro_rules! root_callback {
    ($app:ident, $owner:ident, $callback:ident, |$window:ident $(; $($argument:ident),+)?| $body:expr) => {
        $crate::ui::root_callbacks::root_callback!(
            @target $app, $owner, &$crate::ui::files::file_access(&$owner),
            $callback, |$window $(; $($argument),+)?| $body
        );
    };
    (@target $app:ident, $owner:ident, $target:expr, $callback:ident, |$window:ident $(; $($argument:ident),+)?| $body:expr) => {
        $app.$callback({
            let scope = $crate::ui::admission::Callback::new(&$app, &$owner, $target);
            move |$($($argument),+)?| scope.input((), |$window, $owner| {
                let $window = $window.as_weak();
                $body
            })
        });
    };
}

pub(super) use root_callback;

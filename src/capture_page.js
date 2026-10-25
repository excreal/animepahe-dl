(() => {
    const storage = name => {
        try { return Object.fromEntries(Object.entries(window[name])); }
        catch (error) { return {error: String(error)}; }
    };
    return JSON.stringify({
        url: location.href, title: document.title, readyState: document.readyState,
        html: document.documentElement.outerHTML, text: document.body?.innerText,
        cookies: document.cookie, localStorage: storage('localStorage'),
        sessionStorage: storage('sessionStorage'),
        performance: performance.getEntries().map(entry => entry.toJSON()),
        frames: [...document.querySelectorAll('iframe')].map(frame => ({src: frame.src})),
        console: window.__webkitCapture || []
    });
})()

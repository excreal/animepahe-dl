(() => {
    const record = (state, reason) => {
        window.__webkitPageState = {state, reason, ready: state === 'ready', title: document.title, url: location.href};
        return state === 'ready';
    };
    if (!/^https?:$/.test(location.protocol)) return record('loading', 'Waiting for an HTTP(S) document');
    const text = document.body?.innerText || '';
    const challengeTitle = /just a moment|attention required|verify you are human|security verification/i.test(document.title);
    if (challengeTitle && /verification successful[.\s]*waiting for/i.test(text)) {
        return record('verified_waiting', 'Manual verification succeeded; destination has not loaded');
    }
    if (challengeTitle) return record('challenge', 'Cloudflare challenge page title');
    const visible = element => element.getClientRects().length > 0 && getComputedStyle(element).visibility !== 'hidden';
    const challenge = document.querySelectorAll('#challenge-running, #challenge-stage, #cf-challenge-running, #challenge-form');
    if ([...challenge].some(visible)) return record('challenge', 'Visible challenge container');
    const tokens = [...document.querySelectorAll('[name="cf-turnstile-response"], [name="g-recaptcha-response"], [name="h-captcha-response"]')];
    const widgets = document.querySelectorAll('.cf-turnstile, .g-recaptcha, .h-captcha, #aws-waf-captcha-container, iframe[src*="challenges.cloudflare.com"], iframe[src*="hcaptcha.com"], iframe[src*="google.com/recaptcha/"]');
    if ([...widgets].some(visible) && !tokens.some(input => input.value.trim())) {
        return record('challenge', 'Visible unresolved CAPTCHA widget');
    }
    if (document.readyState !== 'complete') return record('loading', 'Document is still loading');
    return record('ready', 'Document finished loading with no detected CAPTCHA');
})()

(()=>{var e={784:()=>{"undefined"==typeof chrome&&(self.chrome=self.browser)}},o={};function n(t){var r=o[t];if(void 0!==r)return r.exports;var s=o[t]={exports:{}};return e[t](s,s.exports,n),s.exports}(()=>{"use strict";n(784);const e={NODE_ENV:"production",PKG_NAME:"@polkadot/extension",PKG_VERSION:"0.42.2"}.EXTENSION_PREFIX||"",o=`${e}content`,t=`${e}page`,r=`${e}content`,s=chrome.runtime.connect({name:o});s.onMessage.addListener((e=>{window.postMessage({...e,origin:r},"*")})),window.addEventListener("message",(({data:e,source:o})=>{o===window&&e.origin===t&&s.postMessage(e)}));const d=document.createElement("script");d.src=chrome.extension.getURL("page.js"),d.onload=()=>{d.parentNode&&d.parentNode.removeChild(d)},(document.head||document.documentElement).appendChild(d)})()})();
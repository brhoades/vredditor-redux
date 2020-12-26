import * as proto from './pb/main';
import { Empty } from './google/protobuf/empty';

export type APIError<Values> = {
  error?: string;
} & Partial<Values>;


export const vredditID = (url: string): string | undefined => {
  const res = /v\.redd\.it\/([A-Za-z0-9]+)/.exec(url);

  if (res?.length !== 2) {
    return undefined;
  }

  return res[1];
}

export const getVRedditFromComments = (url: string): Promise<string> => (
  new Promise((resolve, reject) => (
    // Thank you cors anywhere for this awesome hack.
    fetch(`//cors-anywhere.herokuapp.com/${url}`)
      .then((r) => r.text())
      .then((text) => {
        const res = /(https:\/\/v\.redd\.it\/[A-Z0-9a-z]+)/.exec(text);

        if (!res || res?.length < 2) {
          reject(new Error('Failed to find v.redd.it url in link; is it a video?'));
        } else {
          resolve(res[1]);
        }
      })
  ))
);

export const getURLsFromMPD = (xml: string): string[] => {
  const parser = new DOMParser();
  const xmlDoc = parser.parseFromString(xml, 'text/xml');

  return [].slice.call(
    xmlDoc
      .getElementsByTagName('BaseURL')
  ).map(({ nodeValue }) => nodeValue);
};

const getVRedditFromUser = (url: string): Promise<string> => (
  new Promise((resolve, reject) => {
    if (/v\.redd\.it\/([A-Za-z0-9]+)/.test(url)) {
      resolve(url);
    } else if (/(m|old)?\.?reddit\.com\/r\/.*?\/comments/.test(url)) {
      // comments link
      getVRedditFromComments(url)
        .then(resolve)
        .catch(reject);
    } else {
      reject(new Error('Couldn\'t get a reddit link from that. Please email me@brod.es if this is a valid reddit link.'));
    }
  })
);

const server = "ws://127.0.0.1:8080";
let connection: VRWebSocket | null = null;

const wsForURL = (url: string): Promise<string> => new Promise((resolve, reject) => {
  if (connection === null) {
    console.log("connection is not open, connectiong...");
    connection = new VRWebSocket(server);
    connection.onopen = () => {
      console.log("connection opened, recursing");
      wsForURL(url).then(resolve).catch(reject);
    };
    connection.onclose = () => { connection = null; reject("connection with server closed"); };
    return;
  }

  connection.onmessage = (msg) => {
    parseResponse(msg.data).then(({ resp }) => {
      if (resp?.$case === "accepted") {
        const accept = resp.accepted;

        if (accept?.resultInner?.$case === "ok") {
          return wsStartDownload(url, resolve, reject);
        } else if (accept?.resultInner?.$case === "err") {
          return reject(`server rejected handshake: ${proto.Result.toJSON(accept)}`);
        }
      }

      reject(`unknown response from server after handshake: ${proto.TranscodeResp.toJSON({ resp })}`);
    }).catch(reject);
  };

  connection.send(NewRequest.handshake());

  console.log("sent handshake after registering listener");
});

const wsStartDownload = (url: string, rawResolve: (_: string) => void, rawReject: (_: string) => void) => {
  let gotStatus = false; // debounce
  let done = false;
  const resolve = (s: string) => {
    done = true;
    rawResolve(s);
  };
  const reject = (s: string) => {
    done = true;
    rawReject(s);
  };

  if (connection === null) {
    console.error("connection closed");
    return rawReject("connection was closed");
  }

  connection.onmessage = (msg) => (
    parseResponse(msg.data).then(({ resp }) => {
      if (resp === undefined) {
        return reject('response had no resp field');
      }

      console.log(`server message: ${resp}`);
      if (resp?.$case !== "jobStatus") {
        return reject(`unexpected message type, expected state: ${msg}`);
      }
      const { state } = resp.jobStatus;

      if (!state) {
        return reject(`unexpected message type, expected state: ${msg}`);
      }
      const stateTy = state?.$case;

      if (stateTy === "queued") {
        console.log("file queued");
      } else if (state?.$case === "completed") {
        const inner = state?.completed?.resultInner;
        switch (inner?.$case) {
          case "ok":
            console.log(`file COMPLETE: ${inner}`);
            return resolve(inner.ok);
          case "err":
            console.log(`file failed: ${inner}`);
            return reject(inner.err);
          default:
            return reject(`unknown result state for completed file from server: ${inner}`);
        }
      } else if (stateTy === "processing") {
        console.log("file PROCESSING");
      } else if (stateTy === "cancelled") {
        console.log("file cancelled");
        return reject("operation was cancelled");
      } else if (stateTy === "uploading") {
        console.log("file UPLOADING");
      } else if (stateTy === "noJobs") {
        console.log("no file queued");
      } else if (stateTy === "unknown") {
        console.log("unknown file state");
      } else {
        console.log(`unknown state message: ${state}`);
      }

      gotStatus = true;
    }).catch(reject)
  );

  connection.send(NewRequest.transcode(url));

  let statusFn = () => {
    if (connection === null || done) {
      return;
    }

    if (gotStatus) {
      connection.send(NewRequest.status());
    } else {
      console.log("Have not received status, debouncing");
    }

    setTimeout(statusFn, 5000);
  };

  setTimeout(statusFn, 5000);

  console.log(`started download for URL: ${url}`);
};

export const getURLs = (url: string, opts: { rehost: boolean, resolveOnFirst: boolean } = { rehost: false, resolveOnFirst: false }): Promise<string[]> => (
  new Promise((resolve, reject) => (
    (
      opts.rehost
        ? wsForURL(url)
        : getVRedditFromUser(url)
    ).then((url) => {
        let id = vredditID(url);

        if (id === undefined) {
          throw new Error('could not get id from vreddit url');
        }

        const qualities = [
          '1080',
          '720',
          '480',
          '360',
          '96',
        ];
        const mp4Urls = qualities.map((quality) => `https://v.redd.it/${id}/DASH_${quality}.mp4`);
        const urls = [
          ...mp4Urls,
          ...qualities.map((quality) => `https://v.redd.it/${id}/DASH_${quality}`)
        ];

        // dear lord, here we go
        const vid = document.createElement('video');
        loadVideos(urls, [], vid, resolve, opts);
      })
      .catch((err) => reject(err))
  ))
);

// Recursively calls itself based on events from the past vid's loading. If it loads, we know
// it's a good url, if it fails, we don't.
//
// If resolveOnFirst is true, we'll resolve on the first success with one URL. Otherwise,
// we accumate URLs by walking all of urls before resolving.
const loadVideos = (urls: string[], valid: string[], vid: HTMLVideoElement, resolve: (_: string[]) => void, opts: { resolveOnFirst: boolean }) => {
  const [url, ...rem] = urls;
  const once = { once: '' };

  const meta = (_e: Event) => {
    remove();
    clearVideo();

    const newURLs = [...valid, url];

    if (rem.length === 0 || opts.resolveOnFirst) {
      return resolve(newURLs);
    }

    setTimeout(() => {
      loadVideos(rem, newURLs, vid, resolve, opts);
    }, 500);
  };

  const fail = (e: Event) => {
    remove();
    once.once = e.type;
    clearVideo();

    if (rem.length === 0) {
      return resolve(valid);
    }

    setTimeout(() => {
      // debounce since abort could fire with error.
      if (once.once !== e.type) {
        return;
      }

      loadVideos(rem, valid, vid, resolve, opts);
    }, 500);
  };

  const types: { [s: string]: (e: Event) => void } = {
    'abort': fail,
    'error': fail,
    'loadedmetadata': meta,
  };

  const remove = () => (
    Object.keys(types).map((t) => (
      vid.removeEventListener(t, types[t])
    ))
  );

  const clearVideo = () => {
    // TypesScript doesn't allow src to be empty, even though
    // it's perfectly fine and stops the video from loading.
    (vid as any).src = undefined;
  };

  Object.keys(types).map((t) => (
    vid.addEventListener(t, types[t])
  ));

  vid.src = url;
};

const parseResponse = (data: Body): Promise<proto.TranscodeResp> => new Promise((resolve, reject) => (
  data.arrayBuffer().then((data: ArrayBuffer) => {
    const { resp } = proto.TranscodeResp.decode(new Uint8Array(data));
    console.log("message received from server");
    console.dir(resp);

    if (resp?.$case === "error") {
      reject("server errored: " + resp.error);
      return;
    }

    resolve({ resp });
  }).catch(reject)
));


class VRWebSocket {
  private inner: WebSocket;
  constructor(addr: string) {
    this.inner = new WebSocket(addr);

    this.inner.onerror = (v) => {
      if (this.onerror) {
        this.onerror(v);
      }

      console.log(`websocket errored: ${v}`);
    };
    this.inner.onclose = (v) => {
      if (this.onclose) {
        this.onclose(v);
      }

      console.log(`websocket closed: ${v}`);
    };
    this.inner.onmessage = (v) => (this.onmessage && this.onmessage(v));
    this.inner.onopen = (v) => (this.onopen && this.onopen(v));
  }

  public onclose: ((this: VRWebSocket, ev: CloseEvent) => any) | null = null;
  public onerror: ((this: VRWebSocket, ev: Event) => any) | null = null;
  public onmessage: ((this: VRWebSocket, ev: MessageEvent) => any) | null = null;
  public onopen: ((this: VRWebSocket, ev: Event) => any) | null = null;

  public send(data: proto.TranscodeReq) {
    this.inner.send(proto.TranscodeReq.encode(data).finish());
  }
}

const NewRequest = {
  transcode: (url: string): proto.TranscodeReq => ({ req: { $case: 'transcode', transcode: { url } } }),
  handshake: (): proto.TranscodeReq => ({ req: { $case: 'handshake', handshake: Empty } }),
  status: (): proto.TranscodeReq => ({ req: { $case: 'status', status: Empty } }),
};

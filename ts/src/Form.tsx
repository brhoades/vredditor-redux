import React, { useState } from 'react';
import { Form as FinalForm, Field, FormRenderProps } from 'react-final-form';
import { FORM_ERROR } from 'final-form';
import { Cookies, useCookies, withCookies } from 'react-cookie';

import Form from 'react-bootstrap/Form';
import Row from 'react-bootstrap/Row';
import Col from 'react-bootstrap/Col';
import Alert from 'react-bootstrap/Alert';

import FinalFormControl from './FinalFormControl';
import FormSelect from './FormSelect';
import SpinButton from './SpinButton';
import { ScrapeOptions, YoutubeDLOptions } from './ExtraFormOptions';
import * as api from './api';
import * as util from './util';

type Values = {
  url: string;
  conversionMethod: "scrape" | "youtubedl";

  // scraper options
  resolveOnFirst?: boolean;

  // server options
  authz?: string;
  server?: string;
};

type ErrorValues<T> = { [P in keyof T]?: string } & { [FORM_ERROR]?: string };

const onSubmit = (statusCb: (status: string) => void, setURLs: (urls: string[]) => void) => ({ url, conversionMethod, server, ...options }: Values): Promise<ErrorValues<Values>> => (
  new Promise((resolve, _reject) => {
    // persist all options on success
    let promise;

    if (conversionMethod === "youtubedl") {
      let correctProto;
      if (!window.location.protocol || window.location.protocol === "http:") {
        correctProto = "ws:";
      } else {
        correctProto = "wss:";
      }

      // resolve to a secure URL, allowing user overrides if present.
      let resolvedURL = server;
      try {
        const serverURL = new URL(server!);
        if (serverURL.protocol !== correctProto) {
          serverURL.protocol = correctProto;
        }

        resolvedURL = serverURL.toString();
      } catch(e) {
        resolvedURL = `${correctProto}://${server}`;
      }

      promise = api.getHostedURL(url, {
       statusCallback: statusCb,
        ...options,
        server: resolvedURL,
      }).then((url: string) => {
        setURLs([url]);
        resolve({});
      });
    } else {
      promise = api.scrapeRedditURLs(url, {
        statusCallback: statusCb,
        resolveOnFirst: options.resolveOnFirst || false,
      })
      .then((urls: string[]) => {
        setURLs(urls);
        resolve({});
      })
    }

    return promise
      .catch((error: Error | string) => {
        console.error('error when returning promise');
        console.dir(error);

        resolve({
          [FORM_ERROR]: (typeof error === "string") ? error : error.message,
        });
      });
  })
);

type URLProps = { status: string } & FormRenderProps<Values>;

const URLForm = ({
  handleSubmit,
  submitting,
  pristine,
  dirtySinceLastSubmit,
  invalid,
  error,
  submitError,
  submitErrors,
  values,
  status,
}: URLProps ) => {
  const formError = submitError || error;
  const serverError = submitErrors !== undefined && submitErrors[FORM_ERROR] !== undefined;
  // Allow server errors (submitError) to retry without changes.
  const disabledFromUserError = !serverError && !dirtySinceLastSubmit;

  return (
    <Form onSubmit={handleSubmit}>
      { formError && <Alert variant="danger">{`${formError}`}</Alert> }
      <Row sm="12" md="8">
        <Form.Group as={Col} md="4">
          <Form.Label>Conversion Method</Form.Label>
          <Field
            name="conversionMethod"
            component={FormSelect}
            type="select"
          >
            <option value="scrape">Scrape Reddit</option>
            <option value="youtubedl">youtube-dl (private)</option>
          </Field>
        </Form.Group>
      </Row>
      <Row sm="12" md="8">
        <Form.Group as={Col} md="12" lg="8">
          <Form.Label>URL</Form.Label>
          <Field
            component={FinalFormControl}
            name="url"
            type="text"
            placeholder="https://v.redd.it/asdf1234"
          />
          <HelpText conversionMethod={values.conversionMethod} />
        </Form.Group>
      </Row>
      {
        values.conversionMethod !== undefined && (
          <>
            <h4>Additional Settings</h4>
            <Row sm="12" md="8">
              <Form.Group as={Col} md="12" lg="8">
                {
                  values.conversionMethod === "youtubedl" &&
                    <YoutubeDLOptions />
                }
                {
                  values.conversionMethod === "scrape" &&
                    <ScrapeOptions />
                }
              </Form.Group>
            </Row>
          </>
        )
      }

      <Row sm="12" md="8">
        <Col md="8">
          <SpinButton
            variant="primary"
            type="submit"
            message="Convert"
            loading={submitting}
            loadingMessage={status ? `${status}...` : "Starting..."}
            disabled={submitting || pristine || (invalid && disabledFromUserError)}
          />
        </Col>
      </Row>
        </Form>
  );
};

const validate = ({ url, conversionMethod, server, authz }: Values): ErrorValues<Values> => {
  let errors: ErrorValues<Values> = {};
  if (!url || url.length < 3) {
    errors.url = 'A valid URL is required';
  } else {
    try {
      new URL(url);
    } catch (e) {
      errors.url = 'This isn\'t a valid URL. It should be in the format https://v.redd.it/asdf1234xyz';
    }
  }

  if (conversionMethod === 'youtubedl') {
    if (server === undefined) {
      errors.server = 'A server address is required';
    } else if (server.length < 5) {
      errors.server = 'A valid server address is required';
    }
    if (authz === undefined) {
      errors.authz = 'An authorization token required';
    } else {
      try {
        Buffer.from(authz, 'base64');
      } catch (e) {
        errors.authz = 'This authorization token is malformed';
      }
    }
  }

  return errors;
};

const decorators = [util.withCookiePersistence<Values>([
  'conversionMethod',
  'resolveOnFirst',
  'authz',
  'server',
])];

export default withCookies(({ cookies, setURLs }: { cookies: Cookies, setURLs: (urls: string[]) => void }) => {
  // surely there's a better spot for this
  const [status, setStatus] = useState("");
  let defaults = {};

  try {
    defaults = cookies.get<string | undefined>('persist') || '{}';
  } catch (e) {
    console.log("error when getting a cookie");
    console.error(e);
  }

  return (
    <FinalForm
      onSubmit={onSubmit(setStatus, setURLs)}
      validate={validate}
      initialValues={defaults}
      decorators={decorators}
      render={(props: FormRenderProps<Values>) => <URLForm status={status} {...(props as any)} />}
    />
  )
});

const HelpText = ({ conversionMethod }: { conversionMethod: "scrape" | "youtubedl" }) => {
  if (conversionMethod === "scrape") {
    return (
      <Form.Text className="text-muted">
        Provide either a direct v.redd.it link or a link to a video's comments.
      </Form.Text>
    );
  } else if (conversionMethod === "youtubedl") {
    return (
      <Form.Text className="text-muted">
        Video link to any &nbsp;
        <a
          href="https://github.com/ytdl-org/youtube-dl/blob/master/docs/supportedsites.md"
          target="_blank"
          rel="noopener noreferrer"
        >
          youtube-dl supported site
        </a>.
      </Form.Text>
    );
  }

  return null;
}

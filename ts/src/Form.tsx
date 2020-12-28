import React, { useState } from 'react';
import { Form as FinalForm, Field, FormRenderProps } from 'react-final-form';
import { FORM_ERROR } from 'final-form';
import { Cookies, useCookies, withCookies } from 'react-cookie';

import Form from 'react-bootstrap/Form';
import Row from 'react-bootstrap/Row';
import Col from 'react-bootstrap/Col';
import Alert from 'react-bootstrap/Alert';

import FinalFormControl from './FinalFormControl';
import FormCheck from './FormCheck';
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

type ErrorValues = Partial<Values> & {
  [FORM_ERROR]?: string;
};

const onSubmit = (statusCb: (status: string) => void, setURLs: (urls: string[]) => void) => ({ url, conversionMethod, ...options }: Values): Promise<ErrorValues> => (
  new Promise((resolve, _reject) => {
    // persist all options on success
    let promise;

    if (conversionMethod === "youtubedl") {
      promise = api.getHostedURL(url, {
       statusCallback: statusCb,
        ...options,
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
      .catch((error: string) => {
        resolve({
          [FORM_ERROR]: error,
        });
      });
  })
);

type URLProps = { status: string, cookies: Cookies } & FormRenderProps<Values>;

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
  cookies,
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
            <option value="scrape">Scrape reddit for source URL (reddit only)</option>
            <option value="youtubedl">Convert and transcode URL (youtube-dl)</option>
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
                    <YoutubeDLOptions cookies={cookies} />
                }
                {
                  values.conversionMethod === "scrape" &&
                    <ScrapeOptions cookies={cookies} />
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

const validate = ({ url, ...rem }: Values): ErrorValues => {
  if (!url || url.length < 3) {
    return {
      url: 'A valid URL is required',
    };
  }

  try {
    new URL(url);
  } catch (_) {
    return {
      url: 'This isn\'t a valid URL. It should be in the format https://v.redd.it/asdf1234xyz',
    };
  }

  return {};
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
    console.error(e);
  }

  return (
    <FinalForm
      onSubmit={onSubmit(setStatus, setURLs)}
      validate={validate}
      initialValues={defaults}
      decorators={decorators}
      render={(props: FormRenderProps<Values>) => <URLForm status={status} cookies={cookies} {...(props as any)} />}
    />
  )
});

const HelpText = ({ conversionMethod }: { conversionMethod: "scrape" | "youtubedl" }) => {
  if (conversionMethod === "scrape") {
    return (
      <Form.Text className="text-muted">
        Can only use v.redd.it links or reddit comments links.
      </Form.Text>
    );
  } else if (conversionMethod === "youtubedl") {
    return (
      <Form.Text className="text-muted">
        Provide any link to any &nbsp;
        <a href="https://github.com/ytdl-org/youtube-dl/blob/master/docs/supportedsites.md">
          youtube-dl supported site
        </a>.
      </Form.Text>
    );
  }

  return null;
}

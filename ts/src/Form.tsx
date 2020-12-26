import React from 'react';
import { Form as FinalForm, Field, FormRenderProps } from 'react-final-form';
import { FORM_ERROR } from 'final-form';

import Form from 'react-bootstrap/Form';
import Row from 'react-bootstrap/Row';
import Col from 'react-bootstrap/Col';
import Alert from 'react-bootstrap/Alert';

import FinalFormControl from './FinalFormControl';
import FormCheck from './FormCheck';
import SpinButton from './SpinButton';
import {
  getURLs,
} from './api';

type Values = {
  url: string;
  resolveOnFirst: boolean;
  rehost: boolean;
};

type ErrorValues = Partial<Values> & {
  [FORM_ERROR]?: string;
};

const onSubmit = (setURLs: (urls: string[]) => void) => ({ url, rehost, resolveOnFirst }: Values): Promise<ErrorValues> => (
  new Promise((resolve) => {
    getURLs(url, { rehost, resolveOnFirst })
      .then((urls) => {
        setURLs(urls);
        resolve();
      })
      .catch((error) => {
        resolve({
          [FORM_ERROR]: error,
        });
      });
  })
);

const URLForm = ({
  handleSubmit,
  submitting,
  pristine,
  dirtySinceLastSubmit,
  invalid,
  error,
  submitError,
  submitErrors,
}: FormRenderProps<Values>) => {
  const formError = submitError || error;
  const serverError = submitErrors !== undefined && submitErrors[FORM_ERROR] !== undefined;
  // Allow server errors (submitError) to retry without changes.
  const disabledFromUserError = !serverError && !dirtySinceLastSubmit;

  return (
    <Form onSubmit={handleSubmit}>
      { formError && <Alert variant="danger">{`${formError}`}</Alert> }
      <Row sm="12" md="8">
        <Form.Group as={Col} md="12" lg="8">
          <Form.Label>URL</Form.Label>
          <Field
            component={FinalFormControl}
            name="url"
            type="text"
            placeholder="https://v.redd.it/asdf1234"
            defaultValue="https://www.youtube.com/watch?v=HxBPqCHFCcc"
          />
            <Form.Text className="text-muted">
            Provide a direct v.redd.it link or reddit comments link.
          </Form.Text>
        </Form.Group>
        <Form.Group as={Col} md="12" lg="8">
          <Field
            component={FormCheck}
            name="resolveOnFirst"
            label="Return only the highest quality video"
          />
        </Form.Group>
        <Form.Group as={Col} md="12" lg="8">
          <Field
            component={FormCheck}
            name="rehost"
            label="Rehost the video. Please use if other options do not work (e.g. audio is missing)."
          />
        </Form.Group>

        <Col md="8">
          <SpinButton
            variant="primary"
            type="submit"
            message="Convert"
            loading={submitting}
            loadingMessage="Converting..."
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

export default ({ setURLs }: { setURLs: (urls: string[]) => void }) => (
  <FinalForm
    onSubmit={onSubmit(setURLs)}
    validate={validate}
    render={URLForm}
    initialValues={{ resolveOnFirst: true }}
  />
);
